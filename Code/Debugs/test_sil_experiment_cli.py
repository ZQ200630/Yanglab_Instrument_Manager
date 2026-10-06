"""SIL CLI/runner and real-driver boundary tests; no hardware acceptance."""

from __future__ import annotations

import csv
import json
import io
import logging
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import numpy as np

from Code.Utils.common import DeviceFault, DriverState, InstrumentSafetyError
from Code.Utils.gain import GainStatus
from Code.Utils.voltage import VoltageStatus
from Code.Experiments.sil_hysteresis.config import (
    AnalysisConfig,
    DeviceConfig,
    OperationPoint,
    RunConfig,
    ScanConfig,
)
from Code.Experiments.sil_hysteresis.storage import RunStore


def peak_nm(spectrum):
    return float(spectrum.wavelength_nm[np.argmax(spectrum.power_dbm)])


class ManualClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class DriverBoundaryTests(unittest.TestCase):
    """Preserved safety/lifecycle coverage on actual public drivers, not a model."""

    def test_voltage_telemetry_has_all_channels_and_tracks_commands(self):
        from Code.Debugs.test_drivers import connected_voltage_driver, decode_command, make_voltage_frame
        driver, wire = connected_voltage_driver(sleep=lambda _: None)
        commanded = (.2, 1., 2., 3., 4., 5., 6., 7.)
        try:
            driver.ramp_to(commanded)
            np.testing.assert_allclose(decode_command(wire.writes[-1]), commanded, atol=.0005)
            wire.queue_frames([make_voltage_frame(commanded, [.1] * 8)] * 3)
            observed = driver.wait_for_channel(1, .2)
            self.assertIsInstance(observed, VoltageStatus)
            self.assertEqual(observed.voltage_v, commanded)
            self.assertEqual(len(observed.current_ma), 8)
        finally:
            driver.close()

    def test_wait_for_channel_requires_new_matching_telemetry(self):
        from Code.Debugs.test_drivers import connected_voltage_driver, make_voltage_frame
        driver, wire = connected_voltage_driver(sleep=lambda _: None)
        try:
            driver.set_channel(1, 6.5)
            wire.queue_frames([make_voltage_frame([0.] * 8, [0.] * 8),
                               make_voltage_frame([6.5] + [0.] * 7, [0.] * 8)] * 3)
            # Literal 16-bit telemetry frame decodes to 6.4992 V, not the requested 6.5 V.
            self.assertEqual(driver.wait_for_channel(1, 6.5).voltage_v, (6.4992,) + (0.,) * 7)
            with self.assertRaises(ValueError):
                driver.wait_for_status(timeout=0.)
        finally:
            driver.close()

    def test_setup_interlock_and_shutdown_order(self):
        from Code.Debugs.test_drivers import ready_gain_driver
        driver, wire = ready_gain_driver(tec_enabled=False, start_watchdog=False)
        try:
            with self.assertRaises(InstrumentSafetyError):
                driver.enable_current()
        finally:
            driver.close()
        self.assertNotIn(b"STQA000001\r\n", wire.writes)
        self.assertEqual(wire.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])

    def test_gain_returns_immutable_status(self):
        from dataclasses import FrozenInstanceError
        from Code.Debugs.test_drivers import ready_gain_driver
        driver, wire = ready_gain_driver(current_enabled=True, start_watchdog=False)
        try:
            status = driver.status
            self.assertIsInstance(status, GainStatus)
            self.assertEqual((status.temperature_c, status.target_c, status.current_ma),
                             (22., 22., 150.))
            self.assertTrue(status.tec_enabled and status.current_enabled)
            with self.assertRaises(FrozenInstanceError):
                status.current_ma = 0.
        finally:
            driver.close()
        self.assertTrue(status.current_enabled)  # Prior immutable evidence is not rewritten.

    def test_public_states_follow_lifecycle(self):
        from App.tests.driver_fixture import WireOSA, WireVoltage, WireGain
        drivers = (WireOSA("GPIB0::29::INSTR"), WireVoltage("COM990"), WireGain("COM991"))
        try:
            for device in drivers:
                self.assertEqual(device.state, DriverState.DISCONNECTED)
                device.connect()
                self.assertEqual(device.state, DriverState.READY)
        finally:
            for device in reversed(drivers):
                device.close()
        self.assertTrue(all(device.state == DriverState.DISCONNECTED for device in drivers))

    def test_gain_close_disables_current_then_tec_even_from_fault(self):
        from Code.Debugs.test_drivers import ready_gain_driver
        for closing_state in (DriverState.ACTIVE, DriverState.FAULT):
            with self.subTest(state=closing_state):
                driver, wire = ready_gain_driver(current_enabled=True, start_watchdog=False)
                before = driver.status
                driver.state = closing_state
                driver.close()
                self.assertEqual(wire.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
                self.assertFalse(wire.is_open)
                self.assertEqual(driver.state, DriverState.DISCONNECTED)
                self.assertTrue(before.current_enabled and before.tec_enabled)


class RecordingVoltage:
    def __init__(self, device, events):
        self._device = device
        self._events = events
        self.commands = []

    @property
    def state(self):
        return self._device.state

    def __getattr__(self, name):
        return getattr(self._device, name)

    def set_channel(self, channel, voltage):
        command_index = len(self.commands)
        self.commands.append((channel, float(voltage)))
        self._events.append(f"voltage.command.{command_index}")
        return self._device.set_channel(channel, voltage)


class RecordingOSA:
    def __init__(self, device, events, failure_at=None, failure=RuntimeError("OSA failed")):
        self._device = device
        self._events = events
        self._failure_at = failure_at
        self._failure = failure
        self.acquisitions = 0

    @property
    def state(self):
        return self._device.state

    def __getattr__(self, name):
        return getattr(self._device, name)

    def acquire(self, trace="A", timeout=None):
        index = self.acquisitions
        self._events.append(f"osa.acquire.{index}")
        if index == self._failure_at:
            raise self._failure
        self.acquisitions += 1
        return self._device.acquire(trace=trace, timeout=timeout)


class RecordingAttempt:
    def __init__(self, attempt, events):
        self._attempt = attempt
        self._events = events

    def __getattr__(self, name):
        return getattr(self._attempt, name)

    def save_acquisition(self, **kwargs):
        saved = self._attempt.save_acquisition(**kwargs)
        self._events.append(f"store.save.{kwargs['step'].sequence_index}")
        return saved

    def mark_complete(self):
        self._attempt.mark_complete()
        self._events.append("attempt.complete")

    def mark_failed(self, error):
        self._attempt.mark_failed(error)
        self._events.append("attempt.failed")


class RecordingRunStore:
    def __init__(self, store, events):
        self._store = store
        self._events = events
        self.path = store.path
        self.config = store.config
        self.schedule = store.schedule

    def operation_complete(self, index):
        return self._store.operation_complete(index)

    def pending_operation_indices(self):
        return self._store.pending_operation_indices()

    def start_attempt(self, index):
        return RecordingAttempt(self._store.start_attempt(index), self._events)


class RecordingWindow:
    def __init__(self, events):
        self.events = events
        self.updates = 0

    def update(self, result, metadata):
        self.updates += 1
        self.events.append("window.update")


def make_config(root, *, points=3, operation_points=None, display_latest=False):
    return RunConfig(
        devices=DeviceConfig(),
        scan=ScanConfig(points=points, settle_time_s=0.0),
        operation_points=tuple(operation_points or (OperationPoint(22.0, 80.0),)),
        temperature_timeout_s=5.0,
        ld_settle_s=0.0,
        analysis=AnalysisConfig(),
        output_root=Path(root),
        display_latest=display_latest,
    )


def write_test_metrics(result, path):
    path = Path(path)
    path.write_text("voltage_v\n", encoding="utf-8")
    path.with_name("summary.json").write_text(
        json.dumps({"classification": result.classification}), encoding="utf-8"
    )


def build_test_figure(result, metadata):
    return result, metadata


def save_test_figure(figure, base_path):
    base = Path(base_path).with_suffix("")
    paths = tuple(base.with_suffix(suffix) for suffix in (".svg", ".pdf", ".png"))
    for path in paths:
        path.write_text("test figure", encoding="utf-8")
    return paths


def make_recording_bundle(
    clock, *, events=None, failure_at=None, failure=RuntimeError("OSA failed")
):
    from Code.Debugs.runner_fixture import runner_bundle
    bundle = runner_bundle(clock, events)
    bundle.voltage = RecordingVoltage(bundle.voltage, bundle.events)
    bundle.osa = RecordingOSA(bundle.osa, bundle.events, failure_at, failure)
    return bundle


def make_runner(config, store, bundle_factory, clock, **overrides):
    from Code.Experiments.sil_hysteresis.experiment import ExperimentRunner

    options = {
        "clock": clock,
        "sleep": clock.sleep,
        "log": logging.getLogger("sil-hysteresis-test"),
        "metrics_writer": write_test_metrics,
        "figure_builder": build_test_figure,
        "figure_saver": save_test_figure,
    }
    options.update(overrides)
    return ExperimentRunner(config, store, bundle_factory, **options)


class CleanupDevice:
    def __init__(self, name, events, failures=()):
        self.name = name
        self.events = events
        self.failures = set(failures)

    def _event(self, action):
        event = f"{self.name}.{action}"
        self.events.append(event)
        if action in self.failures:
            raise RuntimeError(event)

    def zero(self, emergency=False):
        self._event("zero")

    def disable_current(self):
        self._event("disable_current")

    def disable_tec(self):
        self._event("disable_tec")

    def close(self):
        self._event("close")


class RaisingLogger:
    def info(self, message, *args):
        pass

    def error(self, message, *args):
        raise RuntimeError("logger failed")


class CaptureLogger:
    def __init__(self):
        self.messages = []

    def info(self, message, *args):
        self.messages.append(message % args)

    def error(self, message, *args):
        self.messages.append(message % args)


class ExperimentRunnerTests(unittest.TestCase):
    def test_gain_fault_after_acquisition_stops_before_next_voltage(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=2)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            original_acquire = bundle.osa.acquire

            def acquire_then_fault(*args, **kwargs):
                spectrum = original_acquire(*args, **kwargs)
                bundle.gain.state = DriverState.FAULT
                return spectrum

            bundle.osa.acquire = acquire_then_fault

            with self.assertRaises(DeviceFault):
                make_runner(config, store, lambda: bundle, clock).run()

            first_acquire = events.index("osa.acquire.0")
            self.assertFalse(
                any(
                    event.startswith(("voltage.command.", "store.save."))
                    for event in events[first_acquire + 1 :]
                )
            )
            self.assertIn("emergency_zero", events[first_acquire + 1 :])
            self.assertIn("current_disabled", events[first_acquire + 1 :])
            self.assertIn("tec_disabled", events[first_acquire + 1 :])
            self.assertIn("gain_closed", events[first_acquire + 1 :])
            self.assertIn("voltage_closed", events[first_acquire + 1 :])
            self.assertEqual(events[-1], "osa_closed")

    def test_rolling_eta_includes_nonzero_decreasing_remaining_ramp_delay(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=3)
            store = RunStore.create(config)
            bundle = make_recording_bundle(clock)
            log = CaptureLogger()
            runner = make_runner(config, store, lambda: bundle, clock, log=log)

            runner.run()

            eta_values = [
                float(message.rsplit("eta_s=", 1)[1])
                for message in log.messages
                if message.startswith("Acquisition op=")
            ]
            self.assertEqual(len(eta_values), 5)
            self.assertGreater(eta_values[0], 0.0)
            self.assertTrue(
                all(later < earlier for earlier, later in zip(eta_values, eta_values[1:])),
                eta_values,
            )
            self.assertEqual(eta_values[-1], 0.0)

    def test_one_operation_runs_199_acquisitions_and_saves_before_next_voltage(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=100, display_latest=True)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            window = RecordingWindow(events)
            runner = make_runner(
                config, store, lambda: bundle, clock, latest_window=window
            )

            summary = runner.run()

            self.assertEqual(summary.acquisitions, 199)
            self.assertEqual(bundle.osa.acquisitions, 199)
            for index in range(198):
                self.assertLess(
                    events.index(f"store.save.{index}"),
                    events.index(f"voltage.command.{index + 1}"),
                )
            self.assertEqual(
                events[:3], ["osa_connected", "voltage_connected", "gain_connected"]
            )
            self.assertLess(events.index("gain_connected"), events.index("emergency_zero"))
            self.assertLess(events.index("emergency_zero"), events.index("temperature_set"))
            self.assertLess(events.index("current_disabled"), events.index("temperature_set"))
            self.assertLess(events.index("attempt.complete"), events.index("window.update"))
            self.assertLess(
                max(i for i, event in enumerate(events) if event == "emergency_zero" and i < events.index("window.update")),
                events.index("window.update"),
            )

    def test_each_scan_point_confirms_commanded_channel_before_osa_acquisition(self):
        """Catches accepting an arbitrary fresh telemetry frame before OSA."""

        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=3)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            confirmed = []
            underlying = bundle.voltage._device

            def forbidden_fresh_frame(*args, **kwargs):
                self.fail("runner used wait_for_status instead of target confirmation")

            def confirm_target(channel, target_v, **kwargs):
                confirmed.append((channel, target_v))
                return underlying.wait_for_status()

            bundle.voltage.wait_for_status = forbidden_fresh_frame
            bundle.voltage.wait_for_channel = confirm_target

            summary = make_runner(
                config,
                store,
                lambda: bundle,
                clock,
            ).run()

            self.assertEqual(summary.acquisitions, 5)
            self.assertEqual(
                confirmed,
                [(config.scan.channel, step.voltage_v) for step in store.schedule],
            )

    def test_runner_routes_driver_events_to_the_run_audit_logger(self):
        """Catches leaving voltage recovery evidence on an unobserved NullHandler."""

        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=2)
            store = RunStore.create(config)
            bundle = make_recording_bundle(clock)
            bundle.voltage.log = logging.getLogger("discarded.voltage.log")
            audit_log = logging.getLogger("test.sil.audit")

            make_runner(
                config,
                store,
                lambda: bundle,
                clock,
                log=audit_log,
            ).run()

            self.assertIs(bundle.voltage.log, audit_log)

    def test_audit_log_identifies_each_scan_point_before_voltage_activity(self):
        """Catches communication-recovery records with no current scan context."""

        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=2)
            store = RunStore.create(config)
            bundle = make_recording_bundle(clock)
            audit_log = logging.getLogger("test.sil.scan.context")
            records = []

            class Capture(logging.Handler):
                def emit(self, record):
                    records.append(record.getMessage())

            handler = Capture()
            audit_log.addHandler(handler)
            audit_log.setLevel(logging.INFO)
            try:
                make_runner(
                    config,
                    store,
                    lambda: bundle,
                    clock,
                    log=audit_log,
                ).run()
            finally:
                audit_log.removeHandler(handler)

            scan_records = [
                message
                for message in records
                if message.startswith("Voltage scan command ")
            ]
            self.assertEqual(len(scan_records), len(store.schedule))
            for step, message in zip(store.schedule, scan_records, strict=True):
                self.assertIn("op=0", message)
                self.assertIn(f"direction={step.direction}", message)
                self.assertIn(f"point={step.direction_index}", message)
                self.assertIn(f"requested_v={step.voltage_v:.6f}", message)

    def test_coupling_gate_holds_zero_voltage_with_stable_tec_and_enabled_current(self):
        """Catches moving the gate before stability/current enable or after scan start."""
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=3)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            observed = []

            def coupling_gate(index, point):
                status = bundle.gain.read_status()
                voltage = bundle.voltage.read_status()
                observed.append(
                    (
                        index,
                        point,
                        status.tec_enabled,
                        status.current_enabled,
                        status.current_ma,
                        tuple(voltage.voltage_v),
                    )
                )
                events.append("coupling.gate")

            summary = make_runner(
                config,
                store,
                lambda: bundle,
                clock,
                coupling_gate=coupling_gate,
            ).run()

            self.assertEqual(summary.acquisitions, 5)
            self.assertEqual(
                observed,
                [(0, OperationPoint(22.0, 80.0), True, True, 80.0, (0.0,) * 8)],
            )
            self.assertLess(events.index("stability_wait"), events.index("coupling.gate"))
            self.assertLess(events.index("current_enabled"), events.index("coupling.gate"))
            self.assertLess(events.index("current_enabled"), events.index("current_set"))
            self.assertLess(events.index("current_set"), events.index("coupling.gate"))
            self.assertLess(events.index("coupling.gate"), events.index("voltage.command.0"))
            self.assertLess(events.index("coupling.gate"), events.index("osa.acquire.0"))

    def test_coupling_gate_cancellation_acquires_nothing_and_runs_safe_shutdown(self):
        """Catches cancellation that leaves laser outputs on or begins acquisition."""
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=3)
            events = []
            real_store = RunStore.create(config)
            store = RecordingRunStore(real_store, events)
            bundle = make_recording_bundle(clock, events=events)

            def cancel_gate(index, point):
                events.append("coupling.cancel")
                raise KeyboardInterrupt("operator cancelled coupling")

            runner = make_runner(
                config,
                store,
                lambda: bundle,
                clock,
                coupling_gate=cancel_gate,
            )

            with self.assertRaisesRegex(KeyboardInterrupt, "operator cancelled coupling"):
                runner.run()

            self.assertFalse(any(event.startswith("osa.acquire.") for event in events))
            self.assertFalse(any(event.startswith("voltage.command.") for event in events))
            failed = events.index("attempt.failed")
            shutdown = events[failed + 1 :]
            self.assertLess(shutdown.index("emergency_zero"), shutdown.index("current_disabled"))
            self.assertLess(shutdown.index("current_disabled"), shutdown.index("tec_disabled"))
            self.assertFalse(real_store.operation_complete(0))

    def test_each_saved_spectrum_uses_temperature_refreshed_after_osa_acquire(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=3)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            runner = make_runner(config, store, lambda: bundle, clock)

            summary = runner.run()

            temperature_reads = [
                index for index, event in enumerate(events) if event == "temperature_read"
            ]
            self.assertEqual(len(temperature_reads), 5)
            for acquisition_index, read_position in enumerate(temperature_reads):
                self.assertLess(
                    events.index(f"osa.acquire.{acquisition_index}"), read_position
                )
                self.assertLess(
                    read_position, events.index(f"store.save.{acquisition_index}")
                )
            attempt_path = summary.operations[0].attempt_path
            saved_paths = sorted(attempt_path.glob("forward/*.npz"))
            with np.load(saved_paths[0]) as first, np.load(saved_paths[1]) as second:
                self.assertEqual(float(first["measured_temperature_c"]), 22.0)
                self.assertEqual(float(second["measured_temperature_c"]), 22.0)
                self.assertGreater(
                    float(second["gain_status_received_at"]),
                    float(first["gain_status_received_at"]),
                )

    def test_osa_failure_is_fail_fast_marks_attempt_and_preserves_primary_error(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root, points=10)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events, failure_at=12)
            original_gain_close = bundle.gain.close

            def failing_gain_close():
                original_gain_close()
                raise RuntimeError("cleanup failed")

            bundle.gain.close = failing_gain_close
            runner = make_runner(config, store, lambda: bundle, clock)

            with self.assertRaisesRegex(RuntimeError, "OSA failed"):
                runner.run()

            self.assertIn("attempt.failed", events)
            failed_at = events.index("attempt.failed")
            shutdown = events[failed_at + 1 :]
            self.assertLess(shutdown.index("emergency_zero"), shutdown.index("current_disabled"))
            self.assertLess(shutdown.index("current_disabled"), shutdown.index("tec_disabled"))
            self.assertNotIn("osa.acquire.13", events)

    def test_keyboard_interrupt_still_runs_shutdown(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(
                clock,
                events=events,
                failure_at=0,
                failure=KeyboardInterrupt("operator stop"),
            )
            runner = make_runner(config, store, lambda: bundle, clock)

            with self.assertRaises(KeyboardInterrupt):
                runner.run()

            failed_at = events.index("attempt.failed")
            self.assertIn("emergency_zero", events[failed_at + 1 :])
            self.assertIn("tec_disabled", events[failed_at + 1 :])
            self.assertIn("osa_closed", events[failed_at + 1 :])

    def test_safe_shutdown_attempts_every_step_and_raises_first_failure(self):
        from Code.Experiments.sil_hysteresis.experiment import safe_shutdown

        events = []
        voltage = CleanupDevice("voltage", events, failures=("zero",))
        gain = CleanupDevice("gain", events, failures=("disable_tec",))
        osa = CleanupDevice("osa", events)

        with self.assertRaisesRegex(RuntimeError, "voltage.zero"):
            safe_shutdown(voltage, gain, osa, logging.getLogger("cleanup-test"))

        self.assertEqual(
            events,
            [
                "voltage.zero",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ],
        )

    def test_raising_logger_cannot_abort_cleanup_or_replace_first_action_error(self):
        from Code.Experiments.sil_hysteresis.experiment import safe_shutdown

        events = []
        voltage = CleanupDevice("voltage", events, failures=("zero",))
        gain = CleanupDevice("gain", events, failures=("disable_tec",))
        osa = CleanupDevice("osa", events)

        with self.assertRaisesRegex(RuntimeError, "voltage.zero"):
            safe_shutdown(voltage, gain, osa, RaisingLogger())

        self.assertEqual(
            events,
            [
                "voltage.zero",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ],
        )

    def test_raising_logger_cannot_replace_active_acquisition_error(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events, failure_at=0)
            original_gain_close = bundle.gain.close

            def failing_gain_close():
                original_gain_close()
                raise RuntimeError("cleanup failed")

            bundle.gain.close = failing_gain_close
            runner = make_runner(
                config, store, lambda: bundle, clock, log=RaisingLogger()
            )

            with self.assertRaisesRegex(RuntimeError, "OSA failed"):
                runner.run()

            self.assertIn("voltage_closed", events)
            self.assertIn("osa_closed", events)

    def test_successful_run_surfaces_cleanup_failure_after_all_cleanup_steps(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            original_gain_close = bundle.gain.close

            def failing_gain_close():
                original_gain_close()
                raise RuntimeError("gain close failed")

            bundle.gain.close = failing_gain_close
            runner = make_runner(config, store, lambda: bundle, clock)

            with self.assertRaisesRegex(RuntimeError, "gain close failed"):
                runner.run()

            self.assertIn("voltage_closed", events)
            self.assertIn("osa_closed", events)

    def test_operation_transitions_disable_before_temperature_then_enable_and_ramp(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            points = (OperationPoint(22.0, 80.0), OperationPoint(23.0, 90.0))
            config = make_config(root, points=3, operation_points=points)
            events = []
            store = RecordingRunStore(RunStore.create(config), events)
            bundle = make_recording_bundle(clock, events=events)
            runner = make_runner(config, store, lambda: bundle, clock)

            summary = runner.run()

            self.assertEqual(summary.completed_operations, 2)
            temperature_positions = [i for i, event in enumerate(events) if event == "temperature_set"]
            current_positions = [i for i, event in enumerate(events) if event == "current_set"]
            enabled_positions = [i for i, event in enumerate(events) if event == "current_enabled"]
            self.assertEqual(len(temperature_positions), 2)
            self.assertEqual(len(enabled_positions), 2)
            for position in temperature_positions:
                self.assertIn("current_disabled", events[:position])
                self.assertGreater(
                    max(i for i, event in enumerate(events[:position]) if event == "current_disabled"),
                    max(
                        (i for i, event in enumerate(events[:position]) if event == "current_enabled"),
                        default=-1,
                    ),
                )
            for enabled_position in enabled_positions:
                next_voltage = next(
                    i
                    for i in range(enabled_position + 1, len(events))
                    if events[i].startswith("voltage.command.")
                )
                self.assertTrue(
                    any(enabled_position < position < next_voltage for position in current_positions)
                )
            self.assertEqual(events.count("tec_enabled"), 1)
            second_temperature = temperature_positions[1]
            self.assertNotIn("tec_disabled", events[temperature_positions[0] : second_temperature])
            self.assertIn("tec_disabled", events[second_temperature:])

    def test_resume_skips_complete_operation_and_starts_fresh_attempt_at_zero(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            points = (OperationPoint(22.0, 80.0), OperationPoint(23.0, 90.0))
            config = make_config(root, points=3, operation_points=points)
            first_events = []
            real_store = RunStore.create(config)
            first_store = RecordingRunStore(real_store, first_events)
            first_bundle = make_recording_bundle(
                clock, events=first_events, failure_at=5
            )

            with self.assertRaisesRegex(RuntimeError, "OSA failed"):
                make_runner(config, first_store, lambda: first_bundle, clock).run()

            second_events = []
            resumed_store = RecordingRunStore(
                RunStore.open_existing(real_store.path), second_events
            )
            second_bundle = make_recording_bundle(clock, events=second_events)
            summary = make_runner(
                resumed_store.config,
                resumed_store,
                lambda: second_bundle,
                clock,
            ).run()

            self.assertEqual(summary.skipped_operations, 1)
            self.assertEqual(summary.completed_operations, 1)
            self.assertEqual(second_bundle.voltage.commands[0], (1, 0.0))
            op_one = next(real_store.path.glob("op_001_*"))
            self.assertEqual(
                sorted(path.name for path in op_one.glob("attempt_*")),
                ["attempt_001", "attempt_002"],
            )

    def test_postprocessing_failure_leaves_complete_attempt_and_resume_does_not_connect(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            events = []
            real_store = RunStore.create(config)
            store = RecordingRunStore(real_store, events)
            bundle = make_recording_bundle(clock, events=events)

            def failing_figure_saver(figure, base_path):
                raise RuntimeError("figure failed")

            with self.assertRaisesRegex(RuntimeError, "figure failed"):
                make_runner(
                    config,
                    store,
                    lambda: bundle,
                    clock,
                    figure_saver=failing_figure_saver,
                ).run()

            self.assertTrue(real_store.operation_complete(0))
            connections = []

            def forbidden_bundle_factory():
                connections.append("connected")
                raise AssertionError("postprocessing resume constructed instruments")

            resumed = RunStore.open_existing(real_store.path)
            summary = make_runner(
                resumed.config, resumed, forbidden_bundle_factory, clock
            ).run()

            self.assertEqual(connections, [])
            self.assertEqual(summary.acquisitions, 0)
            self.assertEqual(summary.postprocessed_operations, 1)
            attempt = next(real_store.path.glob("op_000_*/attempt_001"))
            for name in (
                "metrics.csv",
                "summary.json",
                "comparison.svg",
                "comparison.pdf",
                "comparison.png",
            ):
                self.assertTrue((attempt / name).is_file(), name)

    def test_analysis_failure_marks_complete_only_after_validation_and_shuts_down(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            events = []
            real_store = RunStore.create(config)
            store = RecordingRunStore(real_store, events)
            bundle = make_recording_bundle(clock, events=events)

            def failing_analysis(data, analysis_config):
                events.append("analysis.failed")
                raise RuntimeError("analysis failed")

            with self.assertRaisesRegex(RuntimeError, "analysis failed"):
                make_runner(
                    config,
                    store,
                    lambda: bundle,
                    clock,
                    analysis_fn=failing_analysis,
                ).run()

            self.assertTrue(real_store.operation_complete(0))
            self.assertLess(events.index("attempt.complete"), events.index("analysis.failed"))
            self.assertIn("emergency_zero", events[events.index("analysis.failed") + 1 :])
            self.assertIn("tec_disabled", events[events.index("analysis.failed") + 1 :])

    def test_acquisition_failure_marks_attempt_failed_not_complete(self):
        with tempfile.TemporaryDirectory() as root:
            clock = ManualClock()
            config = make_config(root)
            real_store = RunStore.create(config)
            events = []
            store = RecordingRunStore(real_store, events)
            bundle = make_recording_bundle(clock, events=events, failure_at=1)

            with self.assertRaisesRegex(RuntimeError, "OSA failed"):
                make_runner(config, store, lambda: bundle, clock).run()

            attempt = next(real_store.path.glob("op_000_*/attempt_001"))
            status = json.loads((attempt / "status.json").read_text(encoding="utf-8"))
            self.assertEqual(status["state"], "failed")
            self.assertFalse(real_store.operation_complete(0))


def write_cli_config(root, *, gain_serial_number=None, points=3, operation_points=None):
    root = Path(root)
    path = root / "cli_config.json"
    path.write_text(
        json.dumps(
            {
                "devices": {
                    "osa_resource": "GPIB9::2::INSTR",
                    "osa_trace": "B",
                    "voltage_port": "COMV",
                    "gain_port": "COMG",
                    "gain_serial_number": gain_serial_number,
                },
                "scan": {
                    "channel": 2,
                    "v_min": 0.0,
                    "v_max": 1.0,
                    "points": points,
                    "spacing": "v_squared",
                    "settle_time_s": 0.1,
                },
                "gain": {
                    "operation_points": operation_points
                    or [{"temperature_c": 22.0, "current_ma": 80.0}]
                },
                "temperature_timeout_s": 5.0,
                "ld_settle_s": 0.0,
                "analysis": {
                    "signal_window_db": 30.0,
                    "display_floor_dbm": None,
                    "thresholds": None,
                },
                "output_root": str(root / "results"),
                "display_latest": True,
            }
        ),
        encoding="utf-8",
    )
    return path


class CLITests(unittest.TestCase):
    def setUp(self):
        from functools import partial
        from Code.Experiments.sil_hysteresis.experiment import ExperimentRunner
        self.clock = ManualClock()
        replacement = partial(ExperimentRunner, clock=self.clock, sleep=self.clock.sleep)
        self.runner_patch = patch("Code.Experiments.sil_hysteresis.run.ExperimentRunner", replacement)
        self.runner_patch.start()
        self.addCleanup(self.runner_patch.stop)

    def fixture_factories(self):
        from Code.Debugs.runner_fixture import runner_factories
        return runner_factories(self.clock)

    @staticmethod
    def forbidden_factories():
        from Code.Experiments.sil_hysteresis.run import DriverFactories

        def forbidden(**kwargs):
            raise AssertionError(f"instrument factory constructed with {kwargs}")

        return DriverFactories(osa=forbidden, voltage=forbidden, gain=forbidden)

    def test_grid_4x4_config_expands_requested_temperature_major_schedule(self):
        """Catches omitting, duplicating, or reordering a requested hardware point."""
        from Code.Experiments.sil_hysteresis.config import load_config

        config_path = (
            Path(__file__).parents[1]
            / "Experiments"
            / "sil_hysteresis"
            / "grid_4x4.json"
        )
        self.assertTrue(config_path.is_file(), "grid_4x4.json has not been created")
        config = load_config(config_path)

        self.assertEqual(
            tuple((point.temperature_c, point.current_ma) for point in config.operation_points),
            (
                (22.0, 100.0), (22.0, 115.0), (22.0, 130.0), (22.0, 145.0),
                (24.0, 100.0), (24.0, 115.0), (24.0, 130.0), (24.0, 145.0),
                (26.0, 100.0), (26.0, 115.0), (26.0, 130.0), (26.0, 145.0),
                (28.0, 100.0), (28.0, 115.0), (28.0, 130.0), (28.0, 145.0),
            ),
        )
        self.assertEqual(config.scan.points, 21)
        self.assertEqual(config.scan.spacing, "v_squared")
        self.assertEqual(config.scan.settle_time_s, 0.1)

    def test_plan_mode_never_constructs_instruments_and_prints_complete_plan(self):
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root, points=100)
            output = io.StringIO()

            result = main(
                ["--config", str(config_path), "--plan"],
                factories=self.forbidden_factories(),
                output=output,
            )

            self.assertEqual(result, 0)
            self.assertFalse((Path(root) / "results").exists())
            plan = output.getvalue()
            for expected in (
                "Operation 1/1: temperature=22.0 degC, current=80.0 mA",
                "Voltage scan: channel=2, range=0.0-1.0 V, spacing=v_squared, points=100",
                "Acquisitions per operation: 199",
                "Total acquisitions: 199",
                "Settle time per acquisition: 0.1 s",
                "Minimum cumulative voltage-ramp time:",
                "Temperature stability timeout: 5.0 s",
                "LD settle wait per operation: 0.0 s",
                "Current ramp: max_step=1.000 mA, interval=0.100 s",
                "OSA acquisition time: unknown",
                f"Output directory: {Path(root) / 'results'}",
                "Shutdown: voltage emergency zero; Gain current disable; Gain TEC disable; "
                "Gain close; Voltage Source close; OSA close",
            ):
                self.assertIn(expected, plan)

    def test_default_plan_reports_exact_actual_driver_minimum_ramp_wait(self):
        from Code.Experiments.sil_hysteresis.run import main

        output = io.StringIO()

        result = main(
            ["--plan"],
            factories=self.forbidden_factories(),
            output=output,
        )

        self.assertEqual(result, 0)
        self.assertIn("Minimum cumulative voltage-ramp time: 7.200 s", output.getvalue())
        self.assertIn("Total estimated current-ramp time from 3 mA: 7.700 s", output.getvalue())

    def test_plan_ramp_helper_counts_only_waits_between_driver_steps(self):
        from Code.Experiments.sil_hysteresis.run import _minimum_ramp_time

        with tempfile.TemporaryDirectory() as root:
            config = replace(
                make_config(root, points=2),
                scan=ScanConfig(v_min=0.0, v_max=0.25, points=2, settle_time_s=0.0),
            )

            # Each 0.00 -> 0.25 or 0.25 -> 0.00 leg uses three writes and only
            # two mandatory 50 ms waits: 2 legs * 2 waits * 0.05 s = 0.20 s.
            self.assertAlmostEqual(_minimum_ramp_time(config), 0.20)

    def test_plan_ramp_helper_mirrors_driver_ceil_for_floating_point_leg(self):
        from Code.Experiments.sil_hysteresis.run import _minimum_ramp_time

        with tempfile.TemporaryDirectory() as root:
            config = replace(
                make_config(root, points=10),
                scan=ScanConfig(v_min=0.0, v_max=0.9, points=10, settle_time_s=0.0),
            )

            # The V-squared grid contains 0.30000000000000004 V legs. The
            # driver uses ceil(delta / 0.1) exactly, requiring four writes and
            # three inter-write waits for those legs.
            self.assertAlmostEqual(_minimum_ramp_time(config), 0.40)

    def test_declined_and_inexact_confirmation_never_construct_instruments(self):
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root)
            for answer in ("NO", "y", "yes", "run ", "Run", ""):
                with self.subTest(answer=answer):
                    output = io.StringIO()
                    result = main(
                        ["--config", str(config_path)],
                        input_fn=lambda _: answer,
                        factories=self.forbidden_factories(),
                        output=output,
                    )
                    self.assertEqual(result, 0)
                    self.assertIn("Cancelled", output.getvalue())
            self.assertFalse((Path(root) / "results").exists())

    def test_config_and_resume_are_mutually_exclusive_before_config_loading(self):
        from Code.Experiments.sil_hysteresis.config import load_config
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root)
            store = RunStore.create(load_config(config_path))
            nonexistent = Path(root) / "does-not-exist.json"

            with patch("sys.stderr", new=io.StringIO()):
                with self.assertRaises(SystemExit) as caught:
                    main(
                        [
                            "--config",
                            str(nonexistent),
                            "--resume",
                            str(store.path),
                            "--plan",
                        ],
                        factories=self.forbidden_factories(),
                        output=io.StringIO(),
                    )

            self.assertEqual(caught.exception.code, 2)

    def test_resume_rejects_historical_modes_before_authorization_or_construction(self):
        from Code.Experiments.sil_hysteresis.config import load_config
        from Code.Experiments.sil_hysteresis.run import main
        with tempfile.TemporaryDirectory() as root:
            config = load_config(write_cli_config(root, points=2))
            for mode in ("similar", "hysteretic"):
                with self.subTest(mode=mode):
                    store = RunStore.create(config)
                    manifest = store.path / "run.json"
                    record = json.loads(manifest.read_text())
                    record.update(run_mode=mode, provenance={
                        "label": "SIMULATED", "run_mode": mode, "simulated": True})
                    manifest.write_text(json.dumps(record))
                    before = manifest.read_bytes()
                    with self.assertRaisesRegex(ValueError, "resume mode mismatch"):
                        main(["--resume", str(store.path)],
                            input_fn=lambda _: self.fail("Historical run prompted"),
                            factories=self.forbidden_factories(), output=io.StringIO())
                    self.assertEqual(manifest.read_bytes(), before)

    def test_matching_real_resume_can_plan_without_construction(self):
        from Code.Experiments.sil_hysteresis.config import load_config
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config = load_config(write_cli_config(root))
            store = RunStore.create(config, run_mode="real")

            result = main(
                ["--resume", str(store.path), "--plan"],
                input_fn=lambda _: (_ for _ in ()).throw(AssertionError("prompted")),
                factories=self.forbidden_factories(),
                output=io.StringIO(),
            )

            self.assertEqual(result, 0)

    def test_exact_run_constructs_only_public_factories_with_expected_keywords(self):
        from Code.Experiments.sil_hysteresis.run import DriverFactories, main
        from Code.Debugs.runner_fixture import runner_bundle

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root, gain_serial_number=None, points=2)
            output = io.StringIO()
            clock = ManualClock()
            bundle = runner_bundle(clock)
            calls = []
            plan_snapshots = []

            def osa_factory(**kwargs):
                calls.append(("osa", kwargs))
                plan_snapshots.append(output.getvalue())
                return bundle.osa

            def voltage_factory(**kwargs):
                calls.append(("voltage", kwargs))
                return bundle.voltage

            def gain_factory(**kwargs):
                calls.append(("gain", kwargs))
                return bundle.gain

            result = main(
                ["--config", str(config_path), "--no-display"],
                input_fn=lambda _: "RUN",
                factories=DriverFactories(
                    osa=osa_factory,
                    voltage=voltage_factory,
                    gain=gain_factory,
                ),
                output=output,
            )

            self.assertEqual(result, 0)
            self.assertEqual(
                calls,
                [
                    ("osa", {"resource_name": "GPIB9::2::INSTR"}),
                    ("voltage", {"port": "COMV"}),
                    ("gain", {"port": "COMG"}),
                ],
            )
            self.assertNotIn("usb_serial", calls[-1][1])
            self.assertIn("Shutdown: voltage emergency zero", plan_snapshots[0])
            self.assertIn("Operation 1/1", plan_snapshots[0])

    def test_real_coupling_pause_requires_exact_coupled_before_first_acquisition(self):
        """Catches a CLI gate that reconnects devices or scans before COUPLED."""
        from Code.Experiments.sil_hysteresis.run import DriverFactories, main
        from Code.Debugs.runner_fixture import runner_bundle

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root, points=2)
            output = io.StringIO()
            clock = ManualClock()
            bundle = runner_bundle(clock)
            prompts = []
            answers = iter(("RUN", "COUPLED"))

            def input_fn(prompt):
                prompts.append(prompt)
                return next(answers)

            result = main(
                [
                    "--config",
                    str(config_path),
                    "--no-display",
                    "--wait-for-coupling",
                ],
                input_fn=input_fn,
                factories=DriverFactories(
                    osa=lambda **_kwargs: bundle.osa,
                    voltage=lambda **_kwargs: bundle.voltage,
                    gain=lambda **_kwargs: bundle.gain,
                ),
                output=output,
            )

            self.assertEqual(result, 0)
            self.assertEqual(len(prompts), 2)
            self.assertIn("RUN", prompts[0])
            self.assertIn("COUPLED", prompts[1])
            self.assertIn("Coupling gate: enabled", output.getvalue())
            self.assertLess(bundle.events.index("current_enabled"), bundle.events.index("spectrum_acquired"))

    def test_real_first_coupling_pause_prompts_once_for_multiple_operations(self):
        """Catches forgetting the first confirmation and pausing again at later points."""
        from Code.Experiments.sil_hysteresis.run import DriverFactories, main
        from Code.Debugs.runner_fixture import runner_bundle

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(
                root,
                points=2,
                operation_points=[
                    {"temperature_c": 22.0, "current_ma": 80.0},
                    {"temperature_c": 22.0, "current_ma": 90.0},
                ],
            )
            output = io.StringIO()
            clock = ManualClock()
            bundle = runner_bundle(clock)
            prompts = []
            answers = iter(("RUN", "COUPLED"))

            def input_fn(prompt):
                prompts.append(prompt)
                return next(answers)

            try:
                result = main(
                    [
                        "--config",
                        str(config_path),
                        "--no-display",
                        "--wait-for-first-coupling",
                    ],
                    input_fn=input_fn,
                    factories=DriverFactories(
                        osa=lambda **_kwargs: bundle.osa,
                        voltage=lambda **_kwargs: bundle.voltage,
                        gain=lambda **_kwargs: bundle.gain,
                    ),
                    output=output,
                )
            except SystemExit as error:
                self.fail(f"--wait-for-first-coupling was not accepted: {error}")

            self.assertEqual(result, 0)
            self.assertEqual(len(prompts), 2)
            self.assertIn("RUN", prompts[0])
            self.assertIn("COUPLED", prompts[1])
            self.assertIn("Coupling gate: first pending operation only", output.getvalue())
            self.assertEqual(bundle.events.count("spectrum_acquired"), 6)

    def test_inexact_coupling_confirmation_shuts_down_without_acquisition(self):
        """Catches accepting a near-match and scanning while coupling is unfinished."""
        from Code.Experiments.sil_hysteresis.run import DriverFactories, main
        from Code.Debugs.runner_fixture import runner_bundle

        for coupling_answer in ("coupled", "COUPLED ", "STOP", ""):
            with self.subTest(answer=coupling_answer), tempfile.TemporaryDirectory() as root:
                config_path = write_cli_config(root, points=2)
                clock = ManualClock()
                bundle = runner_bundle(clock)
                answers = iter(("RUN", coupling_answer))

                with self.assertRaisesRegex(RuntimeError, "coupling gate was not confirmed"):
                    main(
                        [
                            "--config",
                            str(config_path),
                            "--no-display",
                            "--wait-for-coupling",
                        ],
                        input_fn=lambda _prompt: next(answers),
                        factories=DriverFactories(
                            osa=lambda **_kwargs: bundle.osa,
                            voltage=lambda **_kwargs: bundle.voltage,
                            gain=lambda **_kwargs: bundle.gain,
                        ),
                        output=io.StringIO(),
                    )

                self.assertNotIn("spectrum_acquired", bundle.events)
                self.assertIn("current_disabled", bundle.events)
                self.assertIn("tec_disabled", bundle.events)

    def test_explicit_gain_serial_is_forwarded(self):
        from Code.Experiments.sil_hysteresis.config import load_config
        from Code.Experiments.sil_hysteresis.run import DriverFactories, make_real_bundle

        with tempfile.TemporaryDirectory() as root:
            config = load_config(write_cli_config(root, gain_serial_number="GAIN-42"))
            calls = []

            def record(name):
                def factory(**kwargs):
                    calls.append((name, kwargs))
                    return object()

                return factory

            make_real_bundle(
                config,
                DriverFactories(
                    osa=record("osa"), voltage=record("voltage"), gain=record("gain")
                ),
            )

            self.assertEqual(calls[-1], ("gain", {"port": "COMG", "usb_serial": "GAIN-42"}))

    def test_fixed_input_run_preserves_traceability_and_labels_outputs(self):
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root)
            output = io.StringIO()

            result = main(
                [
                    "--config",
                    str(config_path),
                    "--no-display",
                ],
                input_fn=lambda _: "RUN",
                factories=self.fixture_factories(),
                output=output,
            )

            self.assertEqual(result, 0)
            stdout_lines = [line for line in output.getvalue().splitlines() if line]
            self.assertTrue(stdout_lines)
            self.assertTrue(all("REAL" in line for line in stdout_lines))
            run_path = next((Path(root) / "results").glob("run_*"))
            self.assertFalse((run_path / "SIMULATED").exists())
            run_manifest = json.loads((run_path / "run.json").read_text(encoding="utf-8"))
            self.assertEqual(run_manifest["run_mode"], "real")
            log_text = (run_path / "experiment.log").read_text(encoding="utf-8")
            log_lines = [line for line in log_text.splitlines() if line]
            self.assertTrue(log_lines)
            self.assertTrue(all("REAL" in line for line in log_lines))
            self.assertRegex(
                log_lines[0],
                r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z INFO \[REAL\]",
            )
            acquisition_line = next(
                line for line in log_lines if "Acquisition op=" in line
            )
            for field in (
                "direction=",
                "point=",
                "requested_v=",
                "measured_v=",
                "temperature_c=",
                "osa_s=",
                "completed=",
                "mean_osa_s=",
                "eta_s=",
            ):
                self.assertIn(field, acquisition_line)
            self.assertNotIn("array(", log_text)
            completion = next(
                line for line in log_lines if "Operation artifacts:" in line
            )
            self.assertIn("metrics=", completion)
            self.assertIn("discrepancy_voltages=", completion)
            self.assertIn("classification=OBSERVATION_REQUIRED", completion)

            attempt_path = next(run_path.glob("op_*/attempt_*"))
            spectrum_path = next(attempt_path.glob("forward/*.npz"))
            with np.load(spectrum_path, allow_pickle=False) as spectrum:
                self.assertEqual(str(spectrum["provenance_label"]), "REAL")
                self.assertEqual(str(spectrum["run_mode"]), "real")
                self.assertFalse(bool(spectrum["simulated"]))
            with (attempt_path / "acquisitions.csv").open(
                newline="", encoding="utf-8"
            ) as handle:
                acquisition_row = next(csv.DictReader(handle))
            self.assertEqual(acquisition_row["provenance_label"], "REAL")
            self.assertEqual(acquisition_row["run_mode"], "real")
            summary_payload = json.loads(
                (attempt_path / "summary.json").read_text(encoding="utf-8")
            )
            self.assertEqual(
                summary_payload["provenance"],
                {
                    "label": "REAL",
                    "run_mode": "real",
                    "simulated": False,
                },
            )
            with (attempt_path / "metrics.csv").open(
                newline="", encoding="utf-8"
            ) as handle:
                metric_row = next(csv.DictReader(handle))
            self.assertEqual(metric_row["provenance_label"], "REAL")
            self.assertEqual(metric_row["run_mode"], "real")
            svg_text = (attempt_path / "comparison.svg").read_text(encoding="utf-8")
            self.assertIn("REAL", svg_text)
            self.assertIn("REAL run", svg_text)
            self.assertIn(b"REAL run", (attempt_path / "comparison.pdf").read_bytes())
            from PIL import Image

            with Image.open(attempt_path / "comparison.png") as image:
                self.assertIn("REAL", image.info["Description"])
                self.assertIn("REAL run", image.info["Description"])

    def test_error_console_level_keeps_complete_info_audit_file(self):
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root, points=2)
            output = io.StringIO()

            result = main(
                [
                    "--config",
                    str(config_path),
                    "--no-display",
                    "--log-level",
                    "ERROR",
                ],
                factories=self.fixture_factories(),
                    input_fn=lambda _: "RUN",
                output=output,
            )

            self.assertEqual(result, 0)
            self.assertNotIn("Acquisition op=", output.getvalue())
            run_path = next((Path(root) / "results").glob("run_*"))
            audit = (run_path / "experiment.log").read_text(encoding="utf-8")
            self.assertIn("Acquisition op=", audit)
            self.assertIn("Operation artifacts:", audit)
            self.assertIn("Run complete:", audit)

    def test_resume_uses_stored_run_and_regenerates_missing_postprocessing_only(self):
        from Code.Experiments.sil_hysteresis.run import main
        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root)
            self.assertEqual(main(["--config", str(config_path), "--no-display"],
                factories=self.fixture_factories(), input_fn=lambda _: "RUN", output=io.StringIO()), 0)
            run_path = next((Path(root) / "results").glob("run_*"))
            spectra = tuple(sorted(run_path.glob("op_*/attempt_*/*/*.npz")))
            attempt_path = next(run_path.glob("op_*/attempt_*"))
            (attempt_path / "comparison.png").unlink()
            output = io.StringIO()
            result = main(["--resume", str(run_path), "--no-display"],
                factories=self.forbidden_factories(), input_fn=lambda _: "RUN", output=output)
            self.assertEqual(result, 0)
            self.assertEqual(tuple(sorted(run_path.glob("op_*/attempt_*/*/*.npz"))), spectra)
            self.assertTrue((attempt_path / "comparison.png").is_file())
            self.assertEqual(len(tuple((Path(root) / "results").glob("run_*"))), 1)
            self.assertIn(f"Output directory: {run_path}", output.getvalue())
            self.assertIn("postprocessed=1", output.getvalue())

    def test_no_display_does_not_construct_latest_result_window(self):
        from Code.Experiments.sil_hysteresis.run import main

        with tempfile.TemporaryDirectory() as root:
            config_path = write_cli_config(root)
            with patch(
                "Code.Experiments.sil_hysteresis.experiment.LatestResultWindow",
                side_effect=AssertionError("display window constructed"),
            ):
                result = main(
                    ["--config", str(config_path), "--no-display"],
                    factories=self.fixture_factories(),
                    input_fn=lambda _: "RUN",
                    output=io.StringIO(),
                )

            self.assertEqual(result, 0)

    def test_package_exports_the_cli_entry_points(self):
        import Code.Experiments.sil_hysteresis as package
        from Code.Experiments.sil_hysteresis.run import DriverFactories, main

        self.assertIs(package.cli_main, main)
        self.assertIs(package.DriverFactories, DriverFactories)
