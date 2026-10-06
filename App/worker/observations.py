"""Connection-owned Gain evidence; timestamps never cross the process boundary."""

from __future__ import annotations

import copy
import math
import time
from threading import RLock


READERS = {'temperature_c': 'read_temperature', 'target_c': 'read_target',
           'tec_enabled': 'read_tec_enabled', 'current_ma': 'read_current',
           'current_enabled': 'read_current_enabled'}
AFFECTED = {'set_temperature': ('target_c',), 'set_current': ('current_ma',),
            'enable_tec': ('tec_enabled',),
            'disable_tec': ('tec_enabled', 'current_enabled'),
            'enable_current': ('current_enabled', 'current_ma'),
            'disable_current': ('current_enabled',)}
SWITCHES = ('tec_enabled', 'current_enabled')


def communication_time(observation, now):
    """Extract actual local sample time; viewing cached evidence never renews it."""
    stamp = observation.status.get("observed_monotonic")
    if (type(stamp) not in (int, float) or not math.isfinite(stamp)
            or type(now) not in (int, float) or not math.isfinite(now) or stamp > now):
        return None
    return float(stamp)


class EvidenceStore:
    def __init__(self, connection_id: str, *, clock=time.monotonic):
        self.connection_id = connection_id
        self._clock = clock
        self._lock = RLock()
        self._revision = 0
        self._fields = {name: {'value': None, 'source': None, 'quality': 'unknown',
            'connection_id': connection_id, 'revision': None} for name in READERS}
        self._started = {name: None for name in READERS}
        self._floor = {name: -1 for name in READERS}

    def _next_revision(self):
        with self._lock:
            self._revision += 1
            return self._revision

    def record(self, name, value, started_at, revision):
        with self._lock:
            field = self._fields[name]
            boolean = name in SWITCHES
            if (type(value) is not bool if boolean else
                    type(value) not in (int, float) or not math.isfinite(value)):
                raise ValueError('invalid Gain field value')
            if (type(started_at) not in (int, float) or not math.isfinite(started_at)
                    or started_at > self._clock()):
                raise ValueError('invalid observation start time')
            if type(revision) is not int or revision < 0:
                raise ValueError('invalid observation revision')
            if revision <= self._floor[name]:
                return
            if self._started[name] is not None and started_at < self._started[name]:
                return
            self._revision = max(self._revision, revision)
            self._floor[name] = revision
            self._started[name] = started_at
            field.update(value=value, source=READERS[name], quality='fresh', revision=revision)
            field.pop('reason', None)
            field.pop('error', None)

    def invalidate(self, names, reason):
        with self._lock:
            revision = self._next_revision()
            for name in names:
                self._floor[name] = revision
                self._fields[name].update(quality='unknown', reason=reason)
                self._fields[name].pop('error', None)

    def fail(self, name, message):
        with self._lock:
            self._floor[name] = self._next_revision()
            self._fields[name].update(quality='error', error=message)

    def snapshot(self):
        with self._lock:
            now = self._clock()
            fields = copy.deepcopy(self._fields)
            for name, field in fields.items():
                started = self._started[name]
                age = None if started is None else max(0.0, now - started)
                field['observed_age_s'] = age
                if field['quality'] == 'fresh' and age > 5.0:
                    field['quality'] = 'stale'
            return fields
