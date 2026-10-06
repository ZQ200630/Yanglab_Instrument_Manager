"""Bounded recording-event data contract, not a hardware authenticity proof.

Only the owning Host/driver pipeline may create production events. This module
has no clocks, drivers, transport, storage or authority-admission side effects.
"""

from __future__ import annotations

import datetime
import json
import math
import re


MAX_EVENT_BYTES = 8192
MAX_SEQUENCE = 9007199254740991
_ENVELOPE = frozenset({'v', 'recording_id', 'sequence', 'domain', 'connection_id',
    'epoch', 'driver_kind', 'source_kind', 'kind', 'recorded_utc', 'elapsed_ms',
    'read_interval', 'values', 'details'})
_VALUE = frozenset({'field', 'value', 'unit', 'semantics', 'quality', 'source', 'age_ms'})
_INTERVAL = frozenset({'started_utc', 'ended_utc', 'elapsed_ms', 'time_quality'})
_PRIVATE_KEYS = frozenset({'lease_token', 'ownership_nonce', 'session_id',
                          'token', 'password', 'secret'})


class TelemetryError(ValueError):
    """An event cannot enter the recording pipeline as valid evidence."""


def _require(condition, message):
    if not condition:
        raise TelemetryError(message)


def _keys(value, expected):
    _require(type(value) is dict and value.keys() == expected,
             'Unexpected or missing telemetry fields')


def _choice(value, choices):
    _require(type(value) is str and value in choices, 'Unknown telemetry label')


def _identity(value):
    _require(type(value) is str and re.fullmatch(r'[0-9a-f]{32}', value) is not None,
             'Invalid telemetry identity')


def _integer(value, minimum=0):
    _require(type(value) is int and minimum <= value <= MAX_SEQUENCE,
             'Expected a JS-safe telemetry integer')


def _number(value, *, nonnegative=False):
    _require(type(value) in (int, float) and abs(value) <= MAX_SEQUENCE
             and math.isfinite(value) and (not nonnegative or value >= 0),
             'Expected a finite JS-safe telemetry number')


def _text(value, maximum, *, nullable=False, allow_empty=False):
    if value is None and nullable:
        return
    _require(type(value) is str and int(not allow_empty) <= len(value) <= maximum
             and (value == '' or value.isprintable()),
             'Invalid telemetry text')


def _utc(value):
    _require(type(value) is str and re.fullmatch(
        r'[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3}Z',
        value) is not None, 'Expected a millisecond UTC telemetry timestamp')
    try:
        return datetime.datetime.fromisoformat(value[:-1] + '+00:00')
    except ValueError as error:
        raise TelemetryError('Invalid telemetry UTC calendar time') from error


def _details(value):
    _require(type(value) is dict, 'Telemetry details must be an object')
    pending = [(value, 1)]
    visited = 0
    while pending:
        item, depth = pending.pop()
        visited += 1
        _require(visited <= 256, 'Telemetry metadata node capacity exceeded')
        if type(item) in (dict, list):
            _require(depth <= 6 and len(item) <= 32,
                     'Telemetry metadata depth or entry capacity exceeded')
            if type(item) is dict:
                for key, child in item.items():
                    _text(key, 1024)
                    _require(key.casefold() not in _PRIVATE_KEYS,
                             'Live authority or secret cannot enter telemetry')
                    pending.append((child, depth + 1))
            else:
                pending.extend((child, depth + 1) for child in item)
        elif type(item) is str:
            _text(item, 1024, allow_empty=True)
        elif type(item) in (int, float):
            _number(item)
        else:
            _require(item is None or type(item) is bool,
                     'Unsupported telemetry metadata value')


def _validate(event):
    _keys(event, _ENVELOPE)
    _require(type(event['v']) is int and event['v'] == 1,
             'Unsupported telemetry schema version')
    for key in ('recording_id', 'connection_id'):
        _identity(event[key])
    _integer(event['sequence'], 1)
    _integer(event['epoch'])
    _keys(event['domain'], {'kind', 'id'})
    _choice(event['domain']['kind'], {'device', 'setup'})
    _identity(event['domain']['id'])
    _choice(event['driver_kind'], {'osa', 'voltage', 'gain', 'pm400', 'mdt', 'fiber'})
    _choice(event['source_kind'], {'real'})
    _choice(event['kind'], {'sample', 'operation', 'authority', 'fault'})
    _utc(event['recorded_utc'])
    _number(event['elapsed_ms'], nonnegative=True)
    interval = event['read_interval']
    if interval is not None:
        _keys(interval, _INTERVAL)
        start, end = _utc(interval['started_utc']), _utc(interval['ended_utc'])
        _number(interval['elapsed_ms'], nonnegative=True)
        _choice(interval['time_quality'], {'normal', 'discontinuous'})
        _require(end >= start or interval['time_quality'] == 'discontinuous',
                 'Backward UTC interval requires discontinuous quality')
    values = event['values']
    _require(type(values) is list and len(values) <= 32, 'Telemetry field capacity exceeded')
    fields = set()
    for field in values:
        _keys(field, _VALUE)
        name = field['field']
        _require(type(name) is str and re.fullmatch(r'[a-z][a-z0-9._]{0,63}', name)
                 is not None and name not in fields, 'Invalid or duplicate telemetry field')
        fields.add(name)
        _choice(field['semantics'], {'measured', 'readback', 'requested', 'estimated', 'state'})
        _choice(field['quality'], {'fresh', 'stale', 'unknown', 'error'})
        _text(field['unit'], 16, nullable=True)
        _text(field['source'], 64, nullable=True)
        if field['age_ms'] is not None:
            _number(field['age_ms'], nonnegative=True)
        if field['semantics'] == 'state':
            _require((field['value'] is None or type(field['value']) is bool)
                     and field['unit'] is None, 'Telemetry state must be unitless Boolean or null')
        elif field['value'] is not None:
            _number(field['value'])
        if field['quality'] in {'fresh', 'stale'}:
            _require(all(field[key] is not None for key in ('value', 'source', 'age_ms')),
                     'Fresh or stale telemetry requires value, source and age')
    _details(event['details'])


def _unique(pairs):
    result = {}
    for key, value in pairs:
        _require(key not in result, 'Duplicate telemetry JSON key')
        result[key] = value
    return result


def _constant(_):
    raise TelemetryError('Nonfinite telemetry JSON constant')


def encode_event(event: dict) -> bytes:
    """Produce immutable validated UTF-8 bytes, without coercing observations."""
    try:
        _validate(event)
        payload = json.dumps(event, separators=(',', ':'), ensure_ascii=False,
                             allow_nan=False).encode('utf-8')
        _require(len(payload) <= MAX_EVENT_BYTES, 'Telemetry event exceeds 8192 bytes')
        # Validate the actual serialized snapshot too: caller mutation cannot
        # smuggle an unchecked field between prevalidation and serialization.
        decode_event(payload)
        return payload
    except TelemetryError:
        raise
    except (ValueError, TypeError, RecursionError, RuntimeError) as error:
        raise TelemetryError('Cannot encode telemetry event') from error


def decode_event(payload: bytes) -> dict:
    """Parse a detached strict event; data validity grants no live authority."""
    _require(type(payload) is bytes and 0 < len(payload) <= MAX_EVENT_BYTES,
             'Expected at most 8192 telemetry UTF-8 bytes')
    try:
        event = json.loads(payload.decode('utf-8'), object_pairs_hook=_unique,
                           parse_constant=_constant)
        _validate(event)
        return event
    except TelemetryError:
        raise
    except (ValueError, TypeError, RecursionError) as error:
        raise TelemetryError('Cannot decode telemetry event') from error
