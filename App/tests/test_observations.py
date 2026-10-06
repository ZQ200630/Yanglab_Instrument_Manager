"""Per-field evidence must never turn an old or uncertain sample into fresh data."""

import unittest
import threading
from types import SimpleNamespace

from App.worker.controller import ConsoleController
from App.worker.contracts import Context, MAX_EPOCH, Observation, Request
from dataclasses import replace
from App.tests.driver_fixture import WireGain
from Code.Utils.common import DriverState
from Code.Debugs import test_drivers as driver_tests
from Code.Utils import DeviceFault

from App.worker.observations import EvidenceStore


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.now = [10.0]
        self.store = EvidenceStore('c1', clock=lambda: self.now[0])

    def test_only_successful_field_refreshes_and_age_uses_request_start(self):
        values = {'temperature_c': 24.0, 'target_c': 24.0, 'tec_enabled': True,
                  'current_ma': 55.0, 'current_enabled': True}
        for name, value in values.items():
            self.store.record(name, value, started_at=10.0, revision=1)
        self.now[0] = 15.0
        self.assertEqual(self.store.snapshot()['tec_enabled']['quality'], 'fresh')
        self.now[0] = 16.0
        self.store.record('temperature_c', 24.0, started_at=15.0, revision=2)
        fields = self.store.snapshot()
        self.assertEqual(fields['temperature_c']['observed_age_s'], 1.0)
        self.assertEqual(fields['temperature_c']['quality'], 'fresh')
        for name in values.keys() - {'temperature_c'}:
            self.assertEqual(fields[name]['quality'], 'stale')
            self.assertEqual(fields[name]['value'], values[name])
            self.assertEqual(fields[name]['observed_age_s'], 6.0)
        self.assertNotIn('started_at', fields['temperature_c'])
        self.assertNotIn('received_at', fields['temperature_c'])

    def test_invalidation_and_error_retain_history_and_reject_old_read(self):
        self.store.record('current_enabled', True, 10.0, 1)
        self.store.invalidate(('current_enabled',), 'command_started')
        self.store.record('current_enabled', True, 10.0, 1)
        field = self.store.snapshot()['current_enabled']
        self.assertEqual(field['quality'], 'unknown')
        self.assertIs(field['value'], True)
        self.store.fail('current_enabled', 'read failed')
        self.now[0] = 12.0
        field = self.store.snapshot()['current_enabled']
        self.assertEqual(field['quality'], 'error')
        self.assertEqual(field['observed_age_s'], 2.0)
        self.store.record('current_enabled', False, 12.0, 4)
        self.assertIs(self.store.snapshot()['current_enabled']['value'], False)
        self.assertEqual(self.store.snapshot()['current_enabled']['quality'], 'fresh')

    def test_initial_unknown_fields_and_detached_snapshots(self):
        fields = self.store.snapshot()
        self.assertEqual(len(fields), 5)
        for field in fields.values():
            self.assertEqual(field['quality'], 'unknown')
            self.assertIsNone(field['observed_age_s'])
            self.assertIsNone(field['value'])
            self.assertEqual(field['connection_id'], 'c1')
        fields['tec_enabled']['value'] = False
        self.assertIsNone(self.store.snapshot()['tec_enabled']['value'])

    def test_invalid_values_timestamps_and_revisions_cannot_refresh(self):
        for value, started, revision in [(float('nan'), 10., 2), (float('inf'), 10., 2),
                (24., float('nan'), 2), (24., 11., 2), (24., 10., True),
                (24., 10., -1), (24., 10., 0), (24., 10., 1), (True, 10., 2)]:
            with self.subTest(value=value, started=started, revision=revision):
                store = EvidenceStore('c1', clock=lambda: self.now[0])
                store.record('temperature_c', 23., 9., 1)
                before = store.snapshot()
                try:
                    store.record('temperature_c', value, started, revision)
                except (ValueError, TypeError):
                    pass
                self.assertEqual(store.snapshot(), before)
        with self.assertRaises((ValueError, TypeError)):
            self.store.record('tec_enabled', 0, 10., 1)


class GainEvidenceIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.now = [10.0]
        self.counter = 0

    def controller(self, factory=WireGain):
        controller = ConsoleController(port_enumerator=lambda: (), factories={
            'gain': lambda port: factory(port, clock=lambda: self.now[0])})
        controller._scheduler._clock = lambda: self.now[0]
        self.addCleanup(controller.close)
        return controller

    def send(self, controller, method, **params):
        self.counter += 1
        return controller.submit(Request(str(self.counter), method, {'role': 'gain', **params},
                                         controller.context('gain')))

    def connect(self, controller):
        outcome = self.send(controller, 'connect', resource='COM991', acknowledge_lifecycle=True).result(2)
        self.assertEqual(outcome.phase, 'completed', outcome.error)
        return outcome.result['status']

    def refresh(self, controller):
        with controller._scheduler._condition:
            controller._scheduler._roles['gain'].refresh = True
            controller._scheduler._condition.notify_all()

    def idle(self, controller):
        scheduler = controller._scheduler
        with scheduler._condition:
            self.assertTrue(scheduler._condition.wait_for(lambda:
                not scheduler._roles['gain'].observing and not scheduler._roles['gain'].refresh
                and not scheduler._roles['gain'].readback, timeout=2))

    def test_connection_reads_five_public_fields_and_never_cached_status(self):
        class Gain(WireGain):
            def read_status(self):
                raise AssertionError('cached Gain status is not fresh evidence')
        controller = self.controller(Gain)
        snapshot = self.connect(controller)
        self.assertEqual(set(snapshot['fields']), {'temperature_c', 'target_c', 'tec_enabled',
                                                 'current_ma', 'current_enabled'})
        for field in snapshot['fields'].values():
            self.assertEqual(field['quality'], 'fresh')
            self.assertEqual(field['connection_id'], controller.context('gain').connection_id)
        self.assertNotIn('received_at', snapshot)

    def test_initial_connection_is_unknown_while_first_reader_is_blocked(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            def read_temperature(self):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('test release missing')
                return 24.0
        controller = self.controller(Gain)
        self.addCleanup(release.set)
        future = self.send(controller, 'connect', resource='COM991', acknowledge_lifecycle=True)
        self.assertTrue(entered.wait(1))
        fields = controller.cached_status()['devices']['gain']['fields']
        self.assertTrue(all(field['quality'] == 'unknown' for field in fields.values()))
        self.assertTrue(all(field['observed_age_s'] is None for field in fields.values()))
        release.set()
        self.assertEqual(future.result(2).phase, 'completed')

    def test_cached_age_advances_while_refresh_blocks_and_partial_failure_is_local(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            block = False
            def read_temperature(self):
                if self.block:
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError('test release missing')
                return 25.0 if self.block else 24.0
            def read_target(self):
                if self.block:
                    raise OSError('target unavailable')
                return 24.0
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].block = True
        self.addCleanup(release.set)
        self.now[0] = 16.0
        self.refresh(controller)
        self.assertTrue(entered.wait(1))
        fields = controller.cached_status()['devices']['gain']['fields']
        self.assertEqual(fields['current_enabled']['quality'], 'stale')
        self.assertEqual(fields['current_enabled']['observed_age_s'], 6.0)
        # The direct v2 cache-only route must apply the same live overlay.
        status = controller.submit(Request('status', 'status', {}, None)).result(1).result
        self.assertEqual(status['devices']['gain']['fields']['temperature_c']['quality'], 'stale')
        release.set()
        self.idle(controller)
        fields = controller.cached_status()['devices']['gain']['fields']
        self.assertEqual(fields['temperature_c']['value'], 25.0)
        self.assertEqual(fields['temperature_c']['quality'], 'fresh')
        self.assertEqual(fields['target_c']['value'], 24.0)
        self.assertEqual(fields['target_c']['quality'], 'error')
        self.assertIsNone(controller._scheduler._roles['gain'].healthy_context)

    def test_write_invalidation_visible_before_blocked_call_returns(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            def set_current(self, value):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('test release missing')
                raise OSError('unknown command outcome')
        controller = self.controller(Gain)
        self.connect(controller)
        self.addCleanup(release.set)
        future = self.send(controller, 'action', name='set_current', current_ma=50.0)
        self.assertTrue(entered.wait(1))
        field = controller.cached_status()['devices']['gain']['fields']['current_ma']
        self.assertEqual(field['quality'], 'unknown')
        self.assertEqual(field['value'], 0.0)
        release.set()
        self.assertEqual(future.result(2).phase, 'failed_after_call_started')
        self.assertEqual(controller.cached_status()['devices']['gain']['fields']['current_ma']['quality'], 'unknown')

    def test_enable_current_uses_post_enable_reader_not_pre_enable_or_constant(self):
        class Gain(WireGain):
            def enable_current(self):
                # This case tests controller post-call observation, not the
                # driver's interlock (covered below with the actual driver).
                self.changed = True
                return True
            def read_current(self):
                return 7.25 if getattr(self, 'changed', False) else super().read_current()
        controller = self.controller(Gain)
        self.connect(controller)
        enabled = self.send(controller, 'action', name='enable_current').result(2)
        self.assertEqual(enabled.phase, 'completed', enabled.error)
        self.assertEqual(enabled.result['status']['fields']['current_ma']['value'], 7.25)
        off = self.send(controller, 'action', name='disable_tec').result(2)
        self.assertEqual(off.phase, 'completed', off.error)
        self.assertIs(off.result['result'], False)
        for name in ('current_enabled', 'tec_enabled'):
            self.assertIs(off.result['status']['fields'][name]['value'], False)
            self.assertEqual(off.result['status']['fields'][name]['quality'], 'fresh')

    def test_safety_trip_reader_cannot_publish_internal_boolean_or_complete_round(self):
        class Gain(WireGain):
            trip = False
            def read_tec_enabled(self):
                if self.trip:
                    self.status = replace(self.status, tec_enabled=False, current_enabled=False)
                    self.state = DriverState.FAULT
                    raise DeviceFault('interlock tripped')
                return True
            def read_current_enabled(self):
                if self.trip:
                    raise AssertionError('round must stop after interlock fault')
                return True
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].trip = True
        outcome = self.send(controller, 'action', name='set_current', current_ma=22.0).result(2)
        self.assertEqual(outcome.phase, 'completed_readback_failed')
        fields = controller.cached_status()['devices']['gain']['fields']
        self.assertEqual(fields['tec_enabled']['quality'], 'error')
        self.assertEqual(fields['current_enabled']['quality'], 'unknown')
        self.assertIs(fields['tec_enabled']['value'], True)
        self.assertIs(fields['current_enabled']['value'], True)
        self.assertIsNone(controller._scheduler._roles['gain'].healthy_context)
        self.assertEqual(controller.cached_status()['roles']['gain']['state'], 'FAULT')

    def test_old_on_read_cannot_restore_fresh_after_safety_epoch(self):
        entered, release, off_entered, off_release = (threading.Event() for _ in range(4))
        class Gain(WireGain):
            block = False
            def read_current_enabled(self):
                if self.block and not release.is_set():
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError('test release missing')
                    return True
                return self.status.current_enabled
            def disable_current(self):
                if self.block:
                    off_entered.set()
                    if not off_release.wait(3):
                        raise TimeoutError('test release missing')
                return super().disable_current()
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].block = True
        self.addCleanup(release.set)
        self.addCleanup(off_release.set)
        self.refresh(controller)
        self.assertTrue(entered.wait(1))
        stopped = self.send(controller, 'action', name='disable_current')
        self.assertTrue(off_entered.wait(1))
        release.set()
        scheduler = controller._scheduler
        with scheduler._condition:
            self.assertTrue(scheduler._condition.wait_for(lambda: not scheduler._roles['gain'].observing, timeout=1))
        field = controller.cached_status()['devices']['gain']['fields']['current_enabled']
        self.assertEqual(field['quality'], 'unknown')
        self.assertIs(field['value'], False, 'old On must not mutate live store before aggregate rejection')
        off_release.set()
        self.assertEqual(stopped.result(2).phase, 'completed')
        self.assertEqual(controller.cached_status()['roles']['gain']['state'], 'STOP_HELD')

    def test_required_read_error_faults_before_queued_write_at_field_boundary(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        class Gain(WireGain):
            failing = False
            def read_target(self):
                if self.failing:
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError('test release missing')
                    raise OSError('target read failed')
                return 24.0
            def set_current(self, value):
                calls.append(value)
                return super().set_current(value)
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].failing = True
        self.addCleanup(release.set)
        first = self.send(controller, 'action', name='set_current', current_ma=10.0)
        self.assertTrue(entered.wait(1))
        second = self.send(controller, 'action', name='set_current', current_ma=20.0)
        release.set()
        result = first.result(2)
        self.assertEqual(result.phase, 'completed_readback_failed')
        self.assertIn('target read failed', result.error['message'])
        self.assertEqual(second.result(2).phase, 'superseded_before_call')
        self.assertEqual(calls, [10.0])
        self.assertEqual(controller.cached_status()['roles']['gain']['state'], 'FAULT')
        self.assertEqual(controller.cached_status()['devices']['gain']['fields']['target_c']['quality'], 'error')

    def test_ordinary_write_waits_for_field_boundary_and_old_read_is_not_published(self):
        entered, release, written, write_release = (threading.Event() for _ in range(4))
        class Gain(WireGain):
            block = False
            def read_current(self):
                if self.block and not release.is_set():
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError('test release missing')
                    return 99.0
                return self.status.current_ma
            def set_current(self, value):
                written.set()
                if not write_release.wait(3):
                    raise TimeoutError('test release missing')
                return super().set_current(value)
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].block = True
        self.addCleanup(release.set)
        self.addCleanup(write_release.set)
        self.refresh(controller)
        self.assertTrue(entered.wait(1))
        write = self.send(controller, 'action', name='set_current', current_ma=22.0)
        self.assertFalse(written.wait(0.02))
        release.set()
        self.assertTrue(written.wait(1))
        field = controller.cached_status()['devices']['gain']['fields']['current_ma']
        self.assertEqual(field['value'], 0.0)
        self.assertEqual(field['quality'], 'unknown')
        write_release.set()
        self.assertEqual(write.result(2).result['status']['fields']['current_ma']['value'], 22.0)

    def test_reconnect_same_resource_does_not_reuse_old_connection_evidence(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            block = False
            def read_temperature(self):
                if self.block:
                    entered.set()
                    if not release.wait(3):
                        raise TimeoutError('test release missing')
                return 24.0
        controller = self.controller(Gain)
        before = self.connect(controller)['fields']
        self.send(controller, 'disconnect').result(2)
        Gain.block = True
        self.addCleanup(release.set)
        reconnect = self.send(controller, 'connect', resource='COM991', acknowledge_lifecycle=True)
        self.assertTrue(entered.wait(1))
        fields = controller.cached_status()['devices']['gain']['fields']
        for name, field in fields.items():
            self.assertNotEqual(field['connection_id'], before[name]['connection_id'])
            self.assertIsNone(field['value'])
            self.assertEqual(field['quality'], 'unknown')
        release.set()
        self.assertEqual(reconnect.result(2).phase, 'completed')

    def test_metadata_hook_errors_do_not_strand_status_or_stop_futures(self):
        controller = self.controller()
        self.connect(controller)
        def fail(*args):
            raise RuntimeError('metadata hook failed')
        controller._scheduler._status_overlay = fail
        response = controller.submit(Request('status', 'status', {}, None)).result(1)
        self.assertEqual(response.phase, 'completed')
        self.assertIn('status_error', response.result['devices']['gain'])
        self.assertTrue(all(field['quality'] == 'unknown'
                            for field in response.result['devices']['gain']['fields'].values()))
        controller._scheduler._invalidate_role = fail
        self.assertEqual(self.send(controller, 'action', name='disable_current').result(2).phase, 'completed')

    def test_malformed_round_error_cannot_kill_observer_or_strand_readback(self):
        controller = self.controller()
        self.connect(controller)
        original = controller._scheduler._observe
        controller._scheduler._observe = lambda role, context: Observation({'observation_error': 'invalid'})
        self.addCleanup(setattr, controller._scheduler, '_observe', original)
        outcome = self.send(controller, 'action', name='set_current', current_ma=5.0).result(1)
        self.assertEqual(outcome.phase, 'completed_readback_failed')
        self.assertIn('observation error', outcome.error['message'])
        self.assertTrue(next(thread for thread in controller._scheduler._threads
                             if thread.name == 'observe-gain').is_alive())

    def test_periodic_refresh_runs_at_two_point_five_seconds_without_catchup(self):
        calls = []
        class Gain(WireGain):
            def read_temperature(self):
                calls.append(self._clock())
                return 24.0
        controller = self.controller(Gain)
        self.connect(controller)
        self.idle(controller)
        self.assertEqual(calls, [10.0])
        scheduler = controller._scheduler
        with scheduler._condition:
            self.now[0] = 12.5
            scheduler._condition.notify_all()
            self.assertTrue(scheduler._condition.wait_for(lambda: len(calls) == 2, timeout=1))
        self.idle(controller)
        with scheduler._condition:
            self.now[0] = 100.0
            scheduler._condition.notify_all()
            self.assertTrue(scheduler._condition.wait_for(lambda: len(calls) == 3, timeout=1))
        self.idle(controller)
        self.assertEqual(calls, [10.0, 12.5, 100.0])

    def test_exhausted_epoch_cannot_publish_late_command_or_fresh_fields(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            def set_current(self, value):
                entered.set()
                if not release.wait(3):
                    raise TimeoutError('test release missing')
                return super().set_current(value)
        controller = self.controller(Gain)
        self.connect(controller)
        scheduler = controller._scheduler
        with scheduler._condition:
            lane = scheduler._roles['gain']
            lane.context = Context(lane.context.session_id, lane.context.connection_id, MAX_EPOCH)
        self.addCleanup(release.set)
        command = self.send(controller, 'action', name='set_current', current_ma=99.0)
        self.assertTrue(entered.wait(1))
        stop = self.send(controller, 'action', name='disable_current')
        release.set()
        self.assertEqual(command.result(2).phase, 'completed_readback_failed')
        self.assertEqual(stop.result(2).phase, 'completed_readback_failed')
        status = controller.cached_status()['devices']['gain']
        self.assertIsNone(status['last_command'])
        self.assertTrue(all(field['quality'] == 'unknown' for field in status['fields'].values()))

    def test_public_fault_state_invalidates_switches_even_if_numeric_read_returned(self):
        after_fault = []
        class Gain(WireGain):
            fault = False
            def read_temperature(self):
                if self.fault:
                    self.state = DriverState.FAULT
                return 24.0
            def read_target(self):
                if self.fault:
                    after_fault.append('target')
                return super().read_target()
        controller = self.controller(Gain)
        self.connect(controller)
        controller._devices['gain'].fault = True
        self.refresh(controller)
        self.idle(controller)
        fields = controller.cached_status()['devices']['gain']['fields']
        self.assertEqual(fields['tec_enabled']['quality'], 'unknown')
        self.assertEqual(fields['current_enabled']['quality'], 'unknown')
        self.assertEqual(after_fault, [], 'public FAULT ends this observation round')


class GainDriverTests(unittest.TestCase):
    def test_cached_status_does_not_refresh_timestamp_and_readers_are_independent(self):
        now = [10.0]
        gain = WireGain('COM991', clock=lambda: now[0]).connect()
        try:
            before = gain.read_status()
            now[0] = 16.0
            self.assertEqual(gain.read_status().received_at, before.received_at)
            self.assertEqual(gain.read_target(), 22.0)
            self.assertEqual(gain.read_status().received_at, before.received_at)
            self.assertEqual(gain.read_temperature(), 22.0)
            self.assertEqual(gain.read_status().received_at, 16.0)
        finally:
            gain.close()

    def test_boolean_reader_trips_illegal_output_combination_without_success(self):
        for reader in ('read_tec_enabled', 'read_current_enabled'):
            with self.subTest(reader=reader):
                gain = WireGain('COM991').connect()
                try:
                    gain.status = replace(gain.status, current_enabled=True, tec_enabled=False)
                    if reader == 'read_current_enabled':
                        gain.test_wire.replies_by_request[b'RDQA\r\n'] = b'READY;Q=1\r\n'
                    with self.assertRaises(DeviceFault):
                        getattr(gain, reader)()
                    self.assertEqual(gain.state, DriverState.FAULT)
                    self.assertEqual(gain.test_wire.writes[-2:],
                                     [b'STQA000000\r\n', b'STRA000000\r\n'])
                finally:
                    gain.close()

    def test_wait_with_tec_off_times_out_without_enabling_current(self):
        gain = WireGain('COM991').connect()
        try:
            with self.assertRaises(driver_tests.InstrumentTimeoutError):
                gain.wait_stable(timeout=0)
            self.assertNotIn(b'STQA000001\r\n', gain.test_wire.writes)
        finally:
            gain.close()

    def test_tec_off_disables_current_and_post_enable_reads_device_current(self):
        # Preserve both safety checks using their real-driver transport tests;
        # no assumed synthetic current reset value is part of the contract.
        driver_tests.GainFinalReviewTests(
            'test_disable_tec_disables_current_first_and_faults_if_current_is_uncertain').debug()
        driver_tests.GainSafetyReviewTests(
            'test_enable_current_reads_device_reset_current_after_output_turns_on').debug()

    def test_wait_survives_until_close_without_auto_current_enable(self):
        driver_tests.GainSafetyReviewTests(
            'test_close_notifies_wait_stable_while_publishing_closing').debug()

    def test_successful_temperature_wait_does_not_enable_current(self):
        clock = driver_tests.FakeClock()
        gain, wire = driver_tests.ready_gain_driver(start_watchdog=False, clock=clock)
        try:
            wire.queue_temperature(22., 22., 22., 22., 22., 22.)
            for _ in range(6):
                gain._watchdog_iteration()
                clock.advance(1.)
            self.assertIsNone(gain.wait_stable(timeout=0))
            self.assertFalse(gain.status.current_enabled)
            self.assertNotIn(b'STQA000001\r\n', wire.writes)
        finally:
            gain.close()


if __name__ == '__main__':
    unittest.main()
