"""Offline acceptance across the v2 codec, real controller and fixed scheduler.

Only public device calls are replaced; barriers model blocked bench I/O.
"""
from copy import deepcopy
from dataclasses import asdict
import json
from pathlib import Path
import subprocess
import sys
import threading
import unittest

from App.worker.contracts import Context
from App.worker.controller import ConsoleController
from App.worker.protocol import encode_v2, parse_v2
from App.tests.driver_fixture import WireGain, WireOSA, WireVoltage, RESOURCES


class DispatchAcceptanceTests(unittest.TestCase):
    def send(self, controller, request_id, method, context=None, **params):
        if context is None:
            role = params.get('role')
            context = controller.context(role) if role else Context(
                controller.context('osa').session_id, None, 0)
        request = parse_v2(json.dumps(dict(v=2, id=request_id, method=method,
            params=params, context=asdict(context))))
        return controller.submit(request)

    def reply(self, request_id, future):
        reply = json.loads(encode_v2(request_id, future.result(3)))
        self.assertEqual(reply['id'], request_id)
        return reply

    def connect(self, controller, role):
        reply = self.reply('connect-' + role, self.send(controller, 'connect-' + role,
            'connect', role=role, resource=RESOURCES[role], acknowledge_lifecycle=True))
        self.assertTrue(reply['ok'], reply)

    def test_held_osa_ramp_wait_safety_flood_and_polling_keep_final_barriers(self):
        osa_in, osa_out, ramp_in, ramp_out, wait_in, wait_out, zero_in, off_in = (
            threading.Event() for _ in range(8))
        calls = []
        enabled = []

        class OSA(WireOSA):
            def acquire(self, trace='A'):
                osa_in.set()
                if not osa_out.wait(5): raise TimeoutError('OSA barrier not released')
                return super().acquire(trace)

        class Voltage(WireVoltage):
            def set_channel(self, channel, voltage):
                ramp_in.set()
                if not ramp_out.wait(5): raise TimeoutError('ramp barrier not released')
                calls.append(('set', channel, voltage))
                return super().set_channel(channel, voltage)

            def zero(self, emergency=False):
                calls.append(('zero', emergency))
                super().zero(emergency=emergency)
                zero_in.set()

        class Gain(WireGain):
            def wait_stable(self, timeout=60):
                wait_in.set()
                if not wait_out.wait(5): raise TimeoutError('wait barrier not released')
                return True

            def disable_current(self):
                result = super().disable_current()
                off_in.set()
                return result

            def enable_current(self):
                enabled.append('enable_current')
                raise AssertionError('stable wait must never enable current')

        controller = ConsoleController(port_enumerator=lambda: (), factories={
            'osa': OSA, 'voltage': Voltage, 'gain': Gain})
        try:
            for role in ('osa', 'voltage', 'gain'): self.connect(controller, role)
            calls.clear()
            zero_in.clear()
            workers = tuple(controller._scheduler._threads)
            osa = self.send(controller, 'acquire', 'action', role='osa', name='acquire')
            ramp = self.send(controller, 'ramp', 'action', role='voltage',
                name='set_channel', channel=1, voltage=0.1)
            wait = self.send(controller, 'wait', 'action', role='gain', name='wait_stable')
            for event in (osa_in, ramp_in, wait_in): self.assertTrue(event.wait(2))
            queued = self.send(controller, 'queued', 'action', role='voltage',
                name='set_channel', channel=2, voltage=0.2)
            full = self.reply('full', self.send(controller, 'full', 'action', role='voltage',
                name='set_channel', channel=3, voltage=0.3))
            self.assertEqual(full['phase'], 'rejected_before_call')
            zero = self.send(controller, 'zero', 'action', role='voltage', name='zero')
            off = self.send(controller, 'off', 'action', role='gain', name='disable_current')
            self.assertTrue(zero_in.wait(1), 'zero must enter before unrelated OSA release')
            self.assertTrue(off_in.wait(1), 'current-off must enter before unrelated OSA release')
            self.assertFalse(osa_out.is_set())
            self.assertFalse(zero.done(), 'first zero is not the final barrier')
            self.assertFalse(off.done(), 'current-off has not cancelled stable wait')
            self.assertEqual(self.reply('queued', queued)['phase'], 'superseded_before_call')
            for index in range(40):
                for role, name in (('voltage', 'zero'), ('gain', 'disable_current')):
                    request_id = f'{role}-{index}'
                    reply = self.reply(request_id, self.send(controller, request_id,
                        'action', role=role, name=name))
                    self.assertFalse(reply['ok'], reply)
                    self.assertEqual(reply['error']['type'], 'AlreadyRunning')
                request_id = f'poll-{index}'
                status = self.reply(request_id, self.send(controller, request_id, 'status'))
                self.assertTrue(status['ok'])
                self.assertEqual(status['result']['roles']['voltage']['state'], 'STOPPING')
            self.assertEqual(tuple(controller._scheduler._threads), workers)
            self.assertEqual(calls, [('zero', True)])
            ramp_out.set()
            wait_out.set()
            self.reply('ramp', ramp)
            self.reply('wait', wait)
            self.assertEqual(enabled, [], 'even a swallowed enable attempt violates the wait contract')
            self.assertTrue(self.reply('zero', zero)['ok'])
            off_reply = self.reply('off', off)
            self.assertTrue(off_reply['ok'])
            self.assertIs(off_reply['result']['result'], False)
            self.assertEqual(calls, [('zero', True), ('set', 1, 0.1), ('zero', True)])
            self.assertEqual(controller._devices['voltage'].read_status().voltage_v, (0.0,) * 8)
            self.assertFalse(osa.done())
            osa_out.set()
            self.assertTrue(self.reply('acquire', osa)['ok'])
        finally:
            for event in (osa_out, ramp_out, wait_out): event.set()
            controller.close()
            self.assertTrue(controller._scheduler.join(3))

    def test_explicit_shutdown_retry_preserves_report_after_observer_quiesces(self):
        observed, release, closed, failed_close = (threading.Event() for _ in range(4))

        class OSA(WireOSA):
            fail_close = True
            def close(self):
                if self.fail_close:
                    failed_close.set()
                    raise OSError('injected retained VISA resource')
                super().close()
                closed.set()

        controller = ConsoleController(port_enumerator=lambda: (), factories={'osa': OSA})
        original_observe = controller._scheduler._observe
        def held_observe(role, context):
            observed.set()
            if not release.wait(5): raise TimeoutError('observer barrier not released')
            return original_observe(role, context)
        try:
            self.connect(controller, 'osa')
            controller._scheduler._observe = held_observe
            with controller._scheduler._condition:
                controller._scheduler._roles['osa'].refresh = True
                controller._scheduler._condition.notify_all()
            self.assertTrue(observed.wait(2))
            first = self.send(controller, 'shutdown-first', 'shutdown')
            self.assertTrue(failed_close.wait(1))
            self.assertFalse(first.done(), 'failed close still waits for active observation')
            self.assertIn('osa', controller.cached_status()['devices'])
            release.set()
            first_outcome = first.result(3)
            first_report = deepcopy(first_outcome.result)
            self.assertEqual(first_report['unreleased'], ['osa'])
            OSA.fail_close = False
            second = self.send(controller, 'shutdown-second', 'shutdown')
            self.assertTrue(closed.wait(1))
            second_outcome = second.result(3)
            self.assertEqual(second_outcome.result['unreleased'], [])
            self.assertNotEqual(second_outcome.result['attempt_id'], first_report['attempt_id'])
            self.assertEqual(first_outcome.result, first_report)
            self.assertEqual(controller.cached_status()['devices'], {})
        finally:
            OSA.fail_close = False
            release.set()
            controller.close()
            controller.close()  # If an assertion interrupted a failed attempt, explicitly retry.
            self.assertTrue(controller._scheduler.join(3))


class NativeV3FixtureTests(unittest.TestCase):
    def test_native_fixture_is_disarmed_and_keeps_two_instance_contexts(self):
        nonce = 'b' * 32
        child = subprocess.Popen([sys.executable, '-u', '-B',
            str(Path(__file__).with_name('native_worker_fixture.py')), '--real',
            '--protocol', '3', '--ownership-nonce', nonce],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def send(index, method, params=None, context=None):
            child.stdin.write(json.dumps(dict(v=3, id=str(index), method=method,
                params=params or {}, context=context)) + '\n')
            child.stdin.flush()
            return json.loads(child.stdout.readline())
        try:
            startup = send(1, 'ping')
            self.assertEqual(startup['v'], 3)
            self.assertFalse(startup['result']['activated'])
            self.assertEqual(startup['result']['driver_open_calls'], 0)
            self.assertFalse(send(2, 'connect')['ok'])
            self.assertFalse(send(3, 'activate', {'ownership_nonce': 'c' * 32})['ok'])
            self.assertTrue(send(4, 'activate', {'ownership_nonce': nonce})['ok'])
            session = startup['result']['session_id']
            global_context = dict(session_id=session, domain=None, connection_id=None, epoch=0)
            for index in (1, 2):
                domain = dict(kind='device', id=str(index) * 32)
                response = send(4 + index, 'configure_domain',
                    {'config': {'domain': domain, 'config_rev': 1}}, global_context)
                self.assertEqual(response['result']['context']['domain'], domain)
            status = send(7, 'status', context=global_context)
            self.assertEqual(len(status['result']['domains']), 2)
            self.assertEqual(status['result']['driver_open_calls'], 0)
            self.assertTrue(send(8, 'shutdown', context=global_context)['ok'])
            self.assertEqual(child.wait(3), 0)
        finally:
            child.stdin.close()
            if child.poll() is None:
                child.wait(3)
            child.stdout.close()
            child.stderr.close()


if __name__ == '__main__':
    unittest.main()
