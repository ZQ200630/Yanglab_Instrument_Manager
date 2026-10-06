"""Literal recording-event contracts; no instrument, clock or file fixture.

These tests exercise data validation, not authenticity or physical acceptance.
Each rejection catches a malformed sample/authority claim reaching an archive.
"""

import copy
import importlib
import importlib.util
import json
import unittest


def event():
    return {
        'v': 1, 'recording_id': 'a' * 32, 'sequence': 1,
        'domain': {'kind': 'device', 'id': 'b' * 32},
        'connection_id': 'c' * 32, 'epoch': 0, 'driver_kind': 'voltage',
        'source_kind': 'real', 'kind': 'sample',
        'recorded_utc': '2026-10-06T12:30:00.125Z', 'elapsed_ms': 125.5,
        'read_interval': {
            'started_utc': '2026-10-06T12:30:00.100Z',
            'ended_utc': '2026-10-06T12:30:00.125Z',
            'elapsed_ms': 25.0, 'time_quality': 'normal'},
        'values': [
            {'field': 'ch1.voltage', 'value': 0.0, 'unit': 'V',
             'semantics': 'measured', 'quality': 'fresh',
             'source': 'read_status', 'age_ms': 0.0},
            {'field': 'ch1.current', 'value': -0.0025, 'unit': 'mA',
             'semantics': 'measured', 'quality': 'stale',
             'source': 'read_status', 'age_ms': 1250.0}],
        'details': {'state': 'READY', 'channel_count': 8}}


class TelemetrySchemaTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.find_spec('App.worker.telemetry_schema')
        self.assertIsNotNone(spec, 'Missing production telemetry event codec')
        self.codec = importlib.import_module('App.worker.telemetry_schema')

    def reject(self, value):
        with self.assertRaises(self.codec.TelemetryError):
            self.codec.encode_event(value)

    def decode_reject(self, payload):
        with self.assertRaises(self.codec.TelemetryError):
            self.codec.decode_event(payload)

    def test_round_trip_retains_true_zero_negative_current_and_source_age(self):
        original = event()
        encoded = self.codec.encode_event(original)
        self.assertIs(type(encoded), bytes)
        decoded = self.codec.decode_event(encoded)
        self.assertEqual(decoded, original)
        self.assertEqual(decoded['values'][0]['value'], 0.0)
        self.assertEqual(decoded['values'][1]['value'], -0.0025)
        self.assertEqual(decoded['values'][1]['age_ms'], 1250.0)

    def test_unknown_null_is_not_replaced_with_a_fresh_zero(self):
        original = event()
        original['values'][0].update(value=None, quality='unknown', source=None, age_ms=None)
        original['read_interval'] = None
        decoded = self.codec.decode_event(self.codec.encode_event(original))
        self.assertIsNone(decoded['values'][0]['value'])
        self.assertEqual(decoded['values'][0]['quality'], 'unknown')
        self.assertIsNone(decoded['read_interval'])

    def test_boolean_state_is_unitless_and_not_measured_current(self):
        original = event()
        original['driver_kind'] = 'gain'
        original['values'] = [{'field': 'tec_enabled', 'value': False, 'unit': None,
            'semantics': 'state', 'quality': 'fresh', 'source': 'read_tec_enabled', 'age_ms': 0}]
        decoded = self.codec.decode_event(self.codec.encode_event(original))
        self.assertIs(decoded['values'][0]['value'], False)
        original['values'][0]['unit'] = 'mA'
        self.reject(original)
        original['values'][0].update(unit=None, semantics='measured')
        self.reject(original)
        original['values'][0].update(semantics='state', value=1)
        self.reject(original)

    def test_setpoint_and_estimate_semantics_are_not_normalized_to_measurement(self):
        for semantics in ('readback', 'requested', 'estimated'):
            with self.subTest(semantics=semantics):
                original = event()
                original['values'][0]['semantics'] = semantics
                decoded = self.codec.decode_event(self.codec.encode_event(original))
                self.assertEqual(decoded['values'][0]['semantics'], semantics)

    def test_encoder_and_decoder_do_not_share_mutable_input(self):
        original = event()
        encoded = self.codec.encode_event(original)
        original['values'][0]['value'] = 14
        original['details']['state'] = 'FAULT'
        decoded = self.codec.decode_event(encoded)
        self.assertEqual(decoded['values'][0]['value'], 0.0)
        decoded['domain']['id'] = 'd' * 32
        self.assertEqual(self.codec.decode_event(encoded)['domain']['id'], 'b' * 32)

    def test_non_real_source_and_unknown_event_driver_kinds_fail_closed(self):
        for key, value in (('source_kind', 'simulate'), ('source_kind', 'unknown'),
                ('v', 2), ('v', True), ('kind', 'spectrum'), ('driver_kind', 'custom')):
            with self.subTest(key=key, value=value):
                original = event()
                original[key] = value
                self.reject(original)

    def test_ids_sequences_and_epoch_are_strict_js_safe_types(self):
        for key, value in (('recording_id', 'A' * 32), ('recording_id', 'a' * 31),
                ('connection_id', None), ('sequence', 0), ('sequence', True),
                ('sequence', 1.0), ('sequence', 9007199254740992),
                ('epoch', -1), ('epoch', True), ('epoch', 9007199254740992)):
            with self.subTest(key=key, value=value):
                original = event()
                original[key] = value
                self.reject(original)
        original = event()
        original['sequence'] = original['epoch'] = 9007199254740991
        self.assertEqual(self.codec.decode_event(self.codec.encode_event(original))['sequence'],
                         9007199254740991)

    def test_domain_kind_and_unknown_binding_fields_are_rejected(self):
        for domain in ({'kind': 'host', 'id': 'b' * 32},
                {'kind': 'device', 'id': 'B' * 32},
                {'kind': 'setup', 'id': 'b' * 32, 'port': 'COM1'}, None):
            with self.subTest(domain=domain):
                original = event()
                original['domain'] = domain
                self.reject(original)

    def test_unknown_or_missing_schema_fields_cannot_be_silently_ignored(self):
        for target, extra in (('root', 'lease_token'), ('value', 'observed_at'),
                              ('interval', 'instrument_acquired_at')):
            with self.subTest(target=target):
                original = event()
                obj = original if target == 'root' else original['values'][0] if target == 'value' else original['read_interval']
                obj[extra] = 'unexpected'
                self.reject(original)
                obj.pop(extra)
                obj.pop(next(iter(obj)))
                self.reject(original)

    def test_fresh_and_stale_require_non_null_value_source_and_age(self):
        for quality in ('fresh', 'stale'):
            for key in ('value', 'source', 'age_ms'):
                with self.subTest(quality=quality, key=key):
                    original = event()
                    original['values'][0].update(quality=quality)
                    original['values'][0][key] = None
                    self.reject(original)

    def test_field_names_units_sources_and_quality_are_not_coerced(self):
        for key, value in (('field', 'CH1'), ('field', '1.current'), ('field', 'x/y'),
                ('field', ''), ('field', 'x' * 65), ('quality', 'valid'),
                ('semantics', 'command'), ('value', '0'), ('unit', ''),
                ('unit', 'V\n'), ('unit', 'x' * 17), ('source', ''),
                ('source', 'x' * 65), ('age_ms', -1), ('age_ms', True)):
            with self.subTest(key=key, value=value):
                original = event()
                original['values'][0][key] = value
                self.reject(original)

    def test_duplicate_fields_and_more_than_32_fields_are_rejected(self):
        original = event()
        original['values'].append(copy.deepcopy(original['values'][0]))
        self.reject(original)
        original['values'] = [dict(original['values'][0], field=f'ch{index}.voltage')
                              for index in range(1, 34)]
        self.reject(original)
        original['values'].pop()
        self.assertEqual(len(self.codec.decode_event(self.codec.encode_event(original))['values']), 32)

    def test_operation_and_fault_can_retain_context_without_values(self):
        for kind in ('operation', 'authority', 'fault'):
            with self.subTest(kind=kind):
                original = event()
                original.update(kind=kind, values=[], read_interval=None,
                    details={'operation_id': 'd' * 32, 'phase': 'failed_after_call_started'})
                self.assertEqual(self.codec.decode_event(self.codec.encode_event(original))['details'],
                                 {'operation_id': 'd' * 32, 'phase': 'failed_after_call_started'})

    def test_invalid_timestamps_and_negative_elapsed_interval_are_rejected(self):
        for value in ('2026-10-06T12:30:00Z', '2026-10-06T12:30:00.125+00:00',
                      '2026-02-30T12:30:00.125Z', '2026-10-06T25:30:00.125Z', None):
            with self.subTest(value=value):
                original = event()
                original['recorded_utc'] = value
                self.reject(original)
        for key, value in (('elapsed_ms', -1), ('elapsed_ms', True),
                           ('elapsed_ms', 9007199254740992.0)):
            original = event()
            original[key] = value
            self.reject(original)
        original = event()
        original['read_interval']['elapsed_ms'] = -1
        self.reject(original)

    def test_wall_clock_rollback_requires_discontinuous_quality(self):
        original = event()
        original['read_interval']['ended_utc'] = '2026-10-06T12:29:59.000Z'
        self.reject(original)
        original['read_interval']['time_quality'] = 'discontinuous'
        decoded = self.codec.decode_event(self.codec.encode_event(original))
        self.assertEqual(decoded['read_interval']['elapsed_ms'], 25.0)
        self.assertEqual(decoded['read_interval']['ended_utc'], '2026-10-06T12:29:59.000Z')

    def test_nonfinite_and_unsafe_numbers_are_rejected_at_every_location(self):
        for bad in (float('nan'), float('inf'), -float('inf'), 9007199254740992):
            for location in ('value', 'age', 'elapsed', 'details'):
                with self.subTest(bad=bad, location=location):
                    original = event()
                    if location == 'value': original['values'][0]['value'] = bad
                    elif location == 'age': original['values'][0]['age_ms'] = bad
                    elif location == 'elapsed': original['read_interval']['elapsed_ms'] = bad
                    else: original['details']['counter'] = bad
                    self.reject(original)

    def test_live_authority_and_secret_keys_are_rejected_recursively(self):
        for key in ('lease_token', 'ownership_nonce', 'session_id', 'token', 'password', 'secret', 'TOKEN'):
            with self.subTest(key=key):
                original = event()
                original['details'] = {'nested': [{'nested': {key: 'not-for-archive'}}]}
                self.reject(original)

    def test_metadata_type_string_entry_node_and_depth_limits_are_bounded(self):
        for details in ([], {'text': 'x' * 1025}, {'text': 'a\tb'},
                {f'k{index}': index for index in range(33)},
                {'items': list(range(33))}, {'tuple': (1, 2)},
                {'nested': {str(index): list(range(32)) for index in range(8)}}):
            with self.subTest(details=details):
                original = event()
                original['details'] = details
                self.reject(original)
        original = event()
        nested = {}
        original['details'] = nested
        for _ in range(6):
            child = {}
            nested['child'] = child
            nested = child
        self.reject(original)

    def test_empty_optional_device_metadata_is_preserved(self):
        original = event()
        original['details'] = {'friendly_name': '', 'calibration_message': ''}
        try:
            decoded = self.codec.decode_event(self.codec.encode_event(original))
        except self.codec.TelemetryError as error:
            self.fail(f'Valid empty optional metadata was rejected: {error}')
        self.assertEqual(decoded['details'], {'friendly_name': '', 'calibration_message': ''})

    def test_exact_metadata_node_and_container_depth_limits_are_accepted(self):
        original = event()
        # Root + seven lists of (one container +31 scalars) +one list of
        # (one container +30 scalars) =256 visited values, derived by hand.
        original['details'] = {f'list{index}': list(range(31)) for index in range(7)}
        original['details']['last'] = list(range(30))
        self.assertEqual(len(self.codec.decode_event(self.codec.encode_event(original))['details']['last']), 30)
        original['details']['last'].append(30)
        self.reject(original)
        original = event()
        nested = original['details'] = {}
        for _ in range(5):
            child = {}
            nested['child'] = child
            nested = child
        self.assertEqual(self.codec.decode_event(self.codec.encode_event(original))['details'],
                         {'child': {'child': {'child': {'child': {'child': {}}}}}})

    def test_exact_8192_byte_payload_is_accepted_but_next_byte_is_rejected(self):
        original = event()
        original['details'] = {f'pad{index}': 'x' * 1000 for index in range(7)}
        original['details']['last'] = ''
        current_bytes = len(json.dumps(original, ensure_ascii=False, separators=(',', ':')).encode())
        needed = 8192 - current_bytes
        self.assertGreater(needed, 0)
        self.assertLessEqual(needed, 1024)
        original['details']['last'] = 'x' * needed
        self.assertEqual(len(self.codec.encode_event(original)), 8192)
        original['details']['last'] += 'x'
        self.reject(original)

    def test_cycles_and_invalid_unicode_raise_codec_error_not_recursion_error(self):
        original = event()
        original['details']['cycle'] = original['details']
        self.reject(original)
        original = event()
        original['details']['bad'] = '\ud800'
        self.reject(original)
        original = event()
        original['details']['\ud800'] = 'bad key'
        self.reject(original)

    def test_utf8_bytes_not_character_count_bound_an_event(self):
        original = event()
        original['details'] = {f'text{index}': '温' * 500 for index in range(6)}
        text = json.dumps(original, ensure_ascii=False, separators=(',', ':'))
        self.assertLess(len(text), 8192)
        self.assertGreater(len(text.encode('utf-8')), 8192)
        self.reject(original)
        self.decode_reject(text.encode('utf-8'))

    def test_duplicate_root_nested_and_value_json_keys_are_rejected(self):
        encoded = json.dumps(event(), separators=(',', ':')).encode('utf-8')
        self.decode_reject(encoded[:-1] + b',"sequence":2}')
        self.decode_reject(encoded.replace(b'"state":"READY"', b'"state":"READY","state":"FAULT"'))
        self.decode_reject(encoded.replace(b'"value":0.0', b'"value":0.0,"value":14'))

    def test_decoder_checks_schema_even_when_json_is_syntactically_valid(self):
        original = event()
        original['source_kind'] = 'unknown'
        self.decode_reject(json.dumps(original).encode())
        original = event()
        original['values'][0]['quality'] = 'invented'
        self.decode_reject(json.dumps(original).encode())

    def test_decoder_rejects_wrong_type_invalid_json_utf8_constants_and_oversize(self):
        for payload in ('{}', bytearray(b'{}'), b'', b'{}\xff', b'{', b'[]',
                        b'{"v":NaN}', b'{"v":Infinity}', b' ' * 8193,
                        b'{"details":{"text":"\\ud800"}}'):
            with self.subTest(payload=repr(payload)[:70]):
                self.decode_reject(payload)


if __name__ == '__main__':
    unittest.main()
