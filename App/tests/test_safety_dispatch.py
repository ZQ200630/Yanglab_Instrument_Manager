"""Offline tests for early safe-off and final outcome barriers."""

import copy
import threading
import unittest

from App.worker.contracts import Context, Request, MAX_EPOCH, encode_v2
from App.worker.controller import ConsoleController
from App.tests.driver_fixture import WireVoltage, WireGain, WireOSA, WireMeter, EmptyFiber, RESOURCES


class SafetyDispatchTests(unittest.TestCase):
    def connect(self, controller, role):
        return controller.handle('connect', {'role': role, 'resource': RESOURCES[role],
                                            'acknowledge_lifecycle': True})

    def send(self, controller, ident, role, name):
        return controller.submit(Request(ident, 'action', {'role': role, 'name': name},
                                         controller.context(role)))

    def test_standalone_disconnect_history_survives_retry_reconnect_and_status_mutation(self):
        from App.tests.test_controller_transports import ControllerTransportTests
        ControllerTransportTests.test_real_voltage_failed_close_keeps_immutable_attempt_and_can_retry(self)

    def test_upgraded_failed_disconnect_reports_final_intent_to_each_accepted_request(self):
        entered, release = threading.Event(), threading.Event()
        class Gain(WireGain):
            closes = 0
            def disable_current(self):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                return super().disable_current()
            def close(self):
                self.closes += 1
                if self.closes == 1:
                    raise OSError('close failed')
                super().close()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'gain': Gain})
        try:
            self.connect(controller, 'gain')
            first = self.send(controller, 'off', 'gain', 'disable_current')
            self.assertTrue(entered.wait(2))
            stronger = controller.submit(Request('close', 'disconnect', {'role': 'gain'},
                controller.context('gain')))
            expected_attempt = controller.cached_status()['roles']['gain']['safety']['attempt_id']
            release.set()
            for ident, future in [('off', first), ('close', stronger)]:
                outcome = future.result(2)
                self.assertEqual(outcome.phase, 'failed_after_call_started')
                self.assertEqual(outcome.error.get('attempt_id'), expected_attempt)
                self.assertIn('effective_intent=disconnect', outcome.error['message'])
                self.assertIn('close failed', outcome.error['message'])
                encode_v2(ident, outcome)
        finally:
            release.set()
            controller.close()

    def test_final_zero_follows_older_entered_write(self):
        from App.tests.test_controller_transports import ControllerTransportTests
        ControllerTransportTests.test_real_voltage_final_zero_follows_older_entered_command(self)

    def test_waiting_gain_upgrades_to_close_without_claiming_wait_cancelled(self):
        entered, release, off, tec_off, closed = (threading.Event() for _ in range(5))
        class WaitingGain(WireGain):
            def wait_stable(self, timeout=60):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
            def disable_current(self):
                super().disable_current()
                off.set()
                return False
            def disable_tec(self):
                super().disable_tec()
                tec_off.set()
            def close(self):
                super().close()
                closed.set()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'gain': WaitingGain})
        try:
            self.connect(controller, 'gain')
            tec_off.clear()
            old = self.send(controller, 'wait', 'gain', 'wait_stable')
            self.assertTrue(entered.wait(2))
            stop = self.send(controller, 'off', 'gain', 'disable_current')
            self.assertTrue(off.wait(1))
            self.assertFalse(stop.done())
            epoch = controller.context('gain').epoch
            duplicate = self.send(controller, 'off-again', 'gain', 'disable_current').result(1)
            self.assertEqual(duplicate.error['type'], 'AlreadyRunning')
            self.assertEqual(controller.context('gain').epoch, epoch)
            stronger = self.send(controller, 'tec', 'gain', 'disable_tec')
            final = controller.submit(Request('disconnect', 'disconnect', {'role': 'gain'}, controller.context('gain')))
            self.assertTrue(tec_off.wait(1), 'WAITING_OLD must leave safety lane available for TEC off')
            self.assertFalse(closed.is_set(), 'Transport must be retained while the old call runs')
            self.assertFalse(final.done(), 'Disconnect must not claim quiescent release early')
            release.set()
            old.result(2)
            for future in (stop, stronger, final):
                self.assertEqual(future.result(2).result['effective_intent'], 'disconnect')
            self.assertTrue(closed.is_set())
        finally:
            release.set()
            controller.close()

    def test_strict_release_retry_has_independent_reports_and_really_calls_close(self):
        class ContradictoryOSA(WireOSA):
            def __init__(self, **kwargs):
                super().__init__(**kwargs)
                self.is_open, self.resources_released, self.closes = True, False, 0
            def close(self):
                self.closes += 1
                super().close()
                self.resources_released = True
                self.is_open = self.closes == 1
        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': ContradictoryOSA})
        try:
            self.connect(controller, 'osa')
            device = controller._devices['osa']
            first = controller.close()
            preserved = copy.deepcopy(first)
            self.assertEqual(first['unreleased'], ['osa'])
            second = controller.close()
            self.assertEqual(second['unreleased'], [])
            self.assertEqual(device.closes, 2)
            self.assertNotEqual(first['attempt_id'], second['attempt_id'])
            self.assertEqual(first, preserved)
        finally:
            controller.close()

    def test_connect_active_is_closed_only_after_connect_returns(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        class PausedOSA(WireOSA):
            def connect(self):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                super().connect()
                calls.append('connect')
            def close(self):
                calls.append('close')
                super().close()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': PausedOSA})
        try:
            connecting = controller.submit(Request('connect', 'connect', {'role': 'osa', 'resource': RESOURCES['osa'],
                'acknowledge_lifecycle': True}, controller.context('osa')))
            self.assertTrue(entered.wait(2))
            stop = controller.submit(Request('shutdown', 'shutdown', {}, Context(controller._session_id, None, 0)))
            self.assertFalse(stop.done())
            self.assertEqual(calls, [])
            release.set()
            connecting.result(2)
            self.assertEqual(stop.result(2).result['unreleased'], [])
            self.assertEqual(calls, ['connect', 'close'])
        finally:
            release.set()
            controller.close()

    def test_pm400_wait_does_not_delay_gain_cleanup_and_fiber_never_zeroes(self):
        entered, release, gain_closed = (threading.Event() for _ in range(3))
        calls = []
        class BusyMeter(WireMeter):
            def close(self):
                calls.append('pm-close')
                super().close()
        class Gain(WireGain):
            def close(self):
                super().close()
                gain_closed.set()
        class Fiber(EmptyFiber):
            def zero(self, *args, **kwargs):
                raise AssertionError('fiber must never zero')
        controller = ConsoleController(port_enumerator=lambda: (), factories={'pm400': BusyMeter, 'gain': Gain, 'fiber': Fiber})
        original = controller._scheduler._execute
        def execute(req):
            if req.method == 'action' and req.params.get('role') == 'pm400':
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                calls.append('pm-read')
                return {}
            return original(req)
        controller._scheduler._execute = execute
        try:
            self.connect(controller, 'pm400')
            self.connect(controller, 'gain')
            controller.handle('connect', {'role': 'fiber'})
            old = self.send(controller, 'read', 'pm400', 'read')
            self.assertTrue(entered.wait(2))
            shutdown = controller.submit(Request('shutdown', 'shutdown', {}, Context(controller._session_id, None, 0)))
            self.assertTrue(gain_closed.wait(1))
            self.assertFalse(shutdown.done())
            self.assertEqual(calls, [])
            release.set()
            old.result(2)
            self.assertEqual(shutdown.result(2).result['unreleased'], [])
            self.assertEqual(calls, ['pm-read', 'pm-close'])
        finally:
            release.set()
            controller.close()

    def test_failed_off_retries_and_resume_requires_healthy_observation(self):
        observing, release_observer = threading.Event(), threading.Event()
        class Gain(WireGain):
            failures = 1
            def disable_current(self):
                if self.failures:
                    self.failures -= 1
                    raise OSError('off failed')
                return super().disable_current()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'gain': Gain})
        try:
            self.connect(controller, 'gain')
            first = self.send(controller, 'first', 'gain', 'disable_current').result(2)
            self.assertEqual(first.phase, 'failed_after_call_started')
            self.assertEqual(controller.submit(Request('resume-failed', 'resume', {'role': 'gain', 'confirm': True},
                controller.context('gain'))).result(1).phase, 'rejected_before_call')
            second = self.send(controller, 'second', 'gain', 'disable_current').result(2)
            self.assertEqual(second.phase, 'completed')
            self.assertIs(second.result['result'], False)
            scheduler = controller._scheduler
            with scheduler._condition:
                self.assertTrue(scheduler._condition.wait_for(lambda:
                    scheduler._roles['gain'].healthy_context == controller.context('gain')
                    and not scheduler._roles['gain'].observing, timeout=2))
            self.assertEqual(controller.cached_status()['roles']['gain']['state'], 'STOP_HELD')
            reports = controller.cached_status()['roles']['gain']['safety']['attempts']
            self.assertNotEqual(reports[0]['attempt_id'], reports[1]['attempt_id'])
            # A fresh periodic observation may start after the earlier unlocked
            # health check. Resume must still reject while that real slot is held.
            original_observe = scheduler._observe
            def held_observe(role, context):
                observing.set()
                if not release_observer.wait(3):
                    raise TimeoutError('observation barrier not released')
                return original_observe(role, context)
            with scheduler._condition:
                scheduler._observe = held_observe
                scheduler._roles['gain'].refresh = True
                scheduler._condition.notify_all()
            self.assertTrue(observing.wait(2))
            denied = controller.submit(Request('resume-observing', 'resume',
                {'role': 'gain', 'confirm': True}, controller.context('gain'))).result(1)
            self.assertEqual(denied.phase, 'rejected_before_call', denied)
            self.assertEqual(denied.error['type'], 'ResumeRestricted')
            scheduler._observe = original_observe
            release_observer.set()
            # Keep the condition through submit: this is an assertion of the
            # quiescent branch, not a promise that an unlocked status grants it.
            with scheduler._condition:
                self.assertTrue(scheduler._condition.wait_for(lambda:
                    scheduler._roles['gain'].healthy_context == controller.context('gain')
                    and not scheduler._roles['gain'].observing
                    and not scheduler._roles['gain'].readback, timeout=2))
                resumed = controller.submit(Request('resume', 'resume', {'role': 'gain', 'confirm': True},
                    controller.context('gain')))
            outcome = resumed.result(1)
            self.assertEqual(outcome.phase, 'completed', outcome)
        finally:
            release_observer.set()
            controller.close()

    def test_epoch_cap_never_overflows_or_republishes_old_write(self):
        entered, release, zeroed = (threading.Event() for _ in range(3))
        final_entered, final_release = threading.Event(), threading.Event()
        class Voltage(WireVoltage):
            zeros = 0
            def set_channel(self, channel, voltage):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                super().set_channel(channel, voltage)
            def zero(self, emergency=False):
                self.zeros += 1
                if self.zeros == 2:
                    final_entered.set()
                    if not final_release.wait(5):
                        raise TimeoutError('missing final release')
                super().zero(emergency=emergency)
                zeroed.set()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'voltage': Voltage})
        try:
            self.connect(controller, 'voltage')
            with controller._scheduler._condition:
                lane = controller._scheduler._roles['voltage']
                lane.context = Context(lane.context.session_id, lane.context.connection_id, MAX_EPOCH)
            zeroed.clear()
            old = controller.submit(Request('old', 'action', {'role': 'voltage', 'name': 'set_channel',
                'channel': 1, 'voltage': 0.1}, controller.context('voltage')))
            self.assertTrue(entered.wait(2))
            stopped = self.send(controller, 'stop', 'voltage', 'zero')
            self.assertTrue(zeroed.wait(1))
            release.set()
            old.result(2)
            self.assertTrue(final_entered.wait(1))
            self.assertEqual(controller._requested_voltage, [0.0] * 8,
                             'capped epoch must not let old command overwrite zero evidence')
            final_release.set()
            result = stopped.result(2)
            self.assertEqual(result.phase, 'completed_readback_failed')
            self.assertEqual(result.error['type'], 'EpochExhausted')
            encode_v2('stop', result)
            self.assertEqual(controller.context('voltage').epoch, MAX_EPOCH)
            self.assertEqual(self.send(controller, 'more', 'voltage', 'set_all').result(1).error['type'], 'EpochExhausted')
        finally:
            release.set()
            final_release.set()
            controller.close()

    def test_duplicate_off_is_bounded_and_consumer_baseexception_does_not_strand_close(self):
        entered, release = threading.Event(), threading.Event()
        calls = []
        class Gain(WireGain):
            def disable_current(self):
                calls.append('off')
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                return super().disable_current()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'gain': Gain})
        try:
            self.connect(controller, 'gain')
            threads = tuple(controller._scheduler._threads)
            first = self.send(controller, 'first', 'gain', 'disable_current')
            self.assertTrue(entered.wait(2))
            def broken(future):
                raise SystemExit('consumer exited')
            first.add_done_callback(broken)
            later = threading.Event()
            first.add_done_callback(lambda future: later.set())
            for i in range(40):
                result = self.send(controller, str(i), 'gain', 'disable_current').result(1)
                self.assertEqual(result.error['type'], 'AlreadyRunning')
                self.assertTrue(result.error['attempt_id'])
            self.assertEqual(tuple(controller._scheduler._threads), threads)
            self.assertEqual(calls, ['off'])
            release.set()
            self.assertEqual(first.result(2).phase, 'completed')
            self.assertTrue(later.wait(1))
            self.assertEqual(controller.close()['unreleased'], [])
            self.assertTrue(controller._scheduler.join(2))
            self.assertEqual(controller.cached_status()['consumer_callback_errors'][0]['error']['type'], 'SystemExit')
        finally:
            release.set()
            controller.close()

    def test_release_finalizer_failure_retains_identity_and_can_retry(self):
        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': WireOSA, 'gain': WireGain})
        try:
            self.connect(controller, 'osa')
            expected = controller.context('osa').connection_id
            original = controller._scheduler._release_role
            def fail(role, context):
                raise OSError('registry finalization failed')
            controller._scheduler._release_role = fail
            first = controller.close()
            self.assertEqual(first['unreleased'], ['osa'])
            self.assertEqual(controller.context('osa').connection_id, expected)
            self.assertIn('registry finalization failed', controller.cached_status()['devices']['osa']['dispatch_error']['message'])
            controller._scheduler._release_role = original
            self.assertEqual(controller.close()['unreleased'], [])
        finally:
            controller._scheduler._release_role = original
            controller.close()

    def test_release_waits_for_observer_and_same_role_close_is_not_repeated(self):
        entered, release, closed = (threading.Event() for _ in range(3))
        calls = []
        class OSA(WireOSA):
            def close(self):
                calls.append('close')
                super().close()
                closed.set()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': OSA})
        try:
            self.connect(controller, 'osa')
            original = controller._scheduler._observe
            def observe(role, context):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                return original(role, context)
            controller._scheduler._observe = observe
            with controller._scheduler._condition:
                controller._scheduler._roles['osa'].refresh = True
                controller._scheduler._condition.notify_all()
            self.assertTrue(entered.wait(2))
            stop = controller.submit(Request('disconnect', 'disconnect', {'role': 'osa'}, controller.context('osa')))
            self.assertTrue(closed.wait(1))
            self.assertFalse(stop.done())
            self.assertIn('osa', controller._devices)
            duplicate = controller.submit(Request('again', 'disconnect', {'role': 'osa'}, controller.context('osa')))
            self.assertEqual(duplicate.result(1).error['type'], 'AlreadyRunning')
            shutdown = controller.submit(Request('shutdown', 'shutdown', {}, Context(controller._session_id, None, 0)))
            release.set()
            self.assertEqual(stop.result(2).phase, 'completed')
            self.assertEqual(shutdown.result(2).result['unreleased'], [])
            self.assertEqual(calls, ['close'])
        finally:
            release.set()
            controller.close()

    def test_stop_readback_is_superseded_by_close_at_field_boundary(self):
        entered, release = threading.Event(), threading.Event()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': WireOSA, 'gain': WireGain})
        try:
            self.connect(controller, 'gain')
            original = controller._scheduler._observe
            def observe(role, context):
                entered.set()
                if not release.wait(5):
                    raise TimeoutError('missing release')
                return original(role, context)
            controller._scheduler._observe = observe
            off = self.send(controller, 'off', 'gain', 'disable_current')
            self.assertTrue(entered.wait(2))
            close = controller.submit(Request('disconnect', 'disconnect', {'role': 'gain'}, controller.context('gain')))
            release.set()
            self.assertEqual(close.result(2).phase, 'completed')
            self.assertEqual(off.result(2).result['effective_intent'], 'disconnect')
        finally:
            release.set()
            controller.close()

    def test_failed_connect_uses_same_ordered_session_cleanup(self):
        calls = []
        class Gain(WireGain):
            def connect(self):
                super().connect()
                raise OSError('connect failed after opening')
            def disable_current(self):
                calls.append('current_off')
                return super().disable_current()
            def disable_tec(self):
                calls.append('tec_off')
                return super().disable_tec()
            def close(self):
                calls.append('close')
                super().close()
        controller = ConsoleController(port_enumerator=lambda: (), factories={'gain': Gain})
        finalized = []
        original_release = controller._scheduler._release_role
        def finalizer(role, context):
            finalized.append((role, role in controller._devices, list(calls)))
            return original_release(role, context)
        controller._scheduler._release_role = finalizer
        try:
            failed = controller.submit(Request('connect', 'connect', {'role': 'gain', 'resource': RESOURCES['gain'],
                'acknowledge_lifecycle': True}, controller.context('gain'))).result(2)
            self.assertEqual(failed.phase, 'failed_after_call_started')
            self.assertEqual(calls, ['current_off', 'tec_off', 'close'])
            self.assertEqual(finalized, [('gain', True, ['current_off', 'tec_off', 'close'])])
        finally:
            controller.close()
