from __future__ import annotations

import json
import io
import logging
import tempfile
import unittest
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

from Code.Debugs.test_sil_repeat_analysis_figure import config_for
from Code.Experiments.sil_repeat.analysis import write_cycle_products
from Code.Experiments.sil_repeat.experiment import RepeatExperimentRunner
from Code.Experiments.sil_repeat.postprocess import InlinePostprocessor
from Code.Experiments.sil_repeat.scan import cycle_steps
from Code.Experiments.sil_repeat.storage import RepeatRunStore, load_complete_cycle
from Code.Utils.common import DeviceFault, DriverState, InstrumentSafetyError
from Code.Utils.gain import GainStatus
from Code.Utils.osa import Spectrum
from Code.Utils.voltage import VoltageStatus


class FakeOSA:
    def __init__(self, events, fail_at=None, interrupt_at=None):
        self.events = events
        self.fail_at = fail_at
        self.interrupt_at = interrupt_at
        self.count = 0
        self.state = DriverState.DISCONNECTED

    def connect(self):
        self.events.append("osa.connect")
        self.state = DriverState.READY
        return self

    def acquire(self, trace="A"):
        self.count += 1
        self.events.append(f"osa.acquire.{self.count}")
        if self.count == self.fail_at:
            raise RuntimeError("OSA failed")
        if self.count == self.interrupt_at:
            raise KeyboardInterrupt()
        return Spectrum(
            wavelength_nm=np.array([1050.0, 1050.1, 1050.2]),
            power_dbm=np.array([-70.0, -20.0, -60.0]),
            trace=trace,
            acquired_at=datetime(2026, 8, 21, tzinfo=timezone.utc),
            identity="YOKOGAWA,AQ6370E",
        )

    def close(self):
        self.events.append("osa.close")
        self.state = DriverState.DISCONNECTED


class FakeVoltage:
    def __init__(self, events):
        self.events = events
        self.current = 0.0
        self.state = DriverState.DISCONNECTED

    def connect(self):
        self.events.append("voltage.connect")
        self.state = DriverState.READY
        return self

    def zero(self, emergency=False):
        self.current = 0.0
        self.state = DriverState.READY
        self.events.append("voltage.zero.emergency" if emergency else "voltage.zero")

    def set_channel(self, channel, voltage):
        self.current = float(voltage)
        self.state = DriverState.ACTIVE if self.current else DriverState.READY
        self.events.append(f"voltage.set.{channel}.{voltage:.6f}")

    def wait_for_channel(self, channel, voltage):
        self.events.append(f"voltage.wait.{channel}.{voltage:.6f}")
        values = [0.0] * 8
        values[channel - 1] = float(voltage)
        return VoltageStatus(tuple(values), (0.0,) * 8, 42.0)

    def close(self):
        self.events.append("voltage.close")
        self.state = DriverState.DISCONNECTED


class FakeGain:
    def __init__(self, events):
        self.events = events
        self.tec = False
        self.current_enabled = False
        self.target = 24.0
        self.current = 3.0
        self.state = DriverState.DISCONNECTED

    def connect(self):
        self.events.append("gain.connect")
        self.state = DriverState.READY
        return self

    def read_status(self):
        self.events.append("gain.read_status")
        return GainStatus(24.0, self.target, self.tec, self.current, self.current_enabled, 43.0)

    def read_temperature(self):
        self.events.append("gain.read_temperature")
        return 24.0

    def set_temperature(self, value):
        self.target = float(value)
        self.events.append(f"gain.set_temperature.{value:.1f}")
        return self.target

    def enable_tec(self):
        self.tec = True
        self.events.append("gain.enable_tec")
        return True

    def wait_stable(self, timeout):
        self.events.append(f"gain.wait_stable.{timeout:.1f}")

    def enable_current(self):
        self.current_enabled = True
        self.current = 3.0
        self.state = DriverState.ACTIVE
        self.events.append("gain.enable_current")
        return True

    def ramp_current(self, value, step_ma, interval_s):
        self.current = float(value)
        self.events.append(f"gain.ramp_current.{value:.1f}.{step_ma:.1f}.{interval_s:.1f}")

    def disable_current(self):
        self.current_enabled = False
        self.state = DriverState.READY
        self.events.append("gain.disable_current")
        return True

    def disable_tec(self):
        self.tec = False
        self.events.append("gain.disable_tec")
        return True

    def close(self):
        self.events.append("gain.close")
        self.state = DriverState.DISCONNECTED


@dataclass
class FakeBundle:
    osa: FakeOSA
    voltage: FakeVoltage
    gain: FakeGain


def fake_bundle(events, *, fail_at=None, interrupt_at=None):
    return FakeBundle(
        FakeOSA(events, fail_at=fail_at, interrupt_at=interrupt_at),
        FakeVoltage(events),
        FakeGain(events),
    )


class RepeatExperimentTests(unittest.TestCase):
    def test_connect_failure_retains_unreleased_session_without_retry_or_reuse(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)
            primary = RuntimeError("OSA connect failed")
            factory_calls = []

            def failed_connect():
                events.append("osa.connect")
                bundle.osa.state = DriverState.FAULT
                raise primary

            def failed_close():
                events.append("osa.close")
                raise RuntimeError("OSA close failed")

            def factory():
                factory_calls.append("factory")
                return bundle

            bundle.osa.connect = failed_connect
            bundle.osa.close = failed_close
            runner = RepeatExperimentRunner(
                config,
                store,
                factory,
                postprocessor=InlinePostprocessor(lambda request: None),
            )

            with self.assertRaises(RuntimeError) as caught:
                runner.run()

            self.assertIs(caught.exception, primary)
            report = getattr(caught.exception, "cleanup_report", None)
            self.assertIsNotNone(report)
            self.assertEqual(report.unreleased, (("osa", bundle.osa),))
            self.assertIsNotNone(runner._session)
            self.assertEqual(events.count("osa.close"), 1)

            with self.assertRaises(BaseException) as reuse:
                runner.run()

            self.assertIsInstance(reuse.exception, InstrumentSafetyError)
            self.assertIn("unreleased instrument session", str(reuse.exception))
            self.assertEqual(factory_calls, ["factory"])
            self.assertEqual(events.count("osa.close"), 1)

    def test_primary_error_keeps_cleanup_report_and_postprocessor_drain(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)
            primary = RuntimeError("acquisition primary")
            session_during_drain = []

            def failed_acquire(*args, **kwargs):
                events.append("osa.acquire.1")
                raise primary

            bundle.osa.acquire = failed_acquire

            class TrackingPostprocessor(InlinePostprocessor):
                def close_and_drain(self):
                    session_during_drain.append(runner._session)
                    events.append("postprocess.close")
                    return super().close_and_drain()

            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: bundle,
                postprocessor=TrackingPostprocessor(lambda request: None),
                coupling_gate=lambda point: None,
                sleep=lambda seconds: None,
                clock=incrementing_clock(),
            )

            with self.assertRaises(RuntimeError) as caught:
                runner.run()

            self.assertIs(caught.exception, primary)
            report = getattr(caught.exception, "cleanup_report", None)
            self.assertIsNotNone(report)
            self.assertEqual(report.unreleased, ())
            self.assertIsNotNone(session_during_drain[0])
            self.assertIsNotNone(runner._session)
            self.assertEqual(events[-1], "postprocess.close")

    def test_rejecting_report_setter_cannot_replace_primary_or_skip_drain(self):
        class RejectingReportError(RuntimeError):
            def __setattr__(self, name, value):
                if name == "cleanup_report":
                    raise RuntimeError("report setter failed")
                return super().__setattr__(name, value)

        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)
            primary = RejectingReportError("acquisition primary")

            def failed_acquire(*args, **kwargs):
                events.append("osa.acquire.1")
                raise primary

            bundle.osa.acquire = failed_acquire

            class TrackingPostprocessor(InlinePostprocessor):
                def close_and_drain(self):
                    events.append("postprocess.close")
                    return super().close_and_drain()

            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: bundle,
                postprocessor=TrackingPostprocessor(lambda request: None),
                coupling_gate=lambda point: None,
                sleep=lambda seconds: None,
                clock=incrementing_clock(),
            )

            with self.assertRaises(RejectingReportError) as caught:
                runner.run()

            self.assertIs(caught.exception, primary)
            self.assertEqual(events[-1], "postprocess.close")

    def test_unreleased_cleanup_resource_retains_session_and_report(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)

            def failed_gain_close():
                events.append("gain.close")
                raise RuntimeError("gain close failed")

            bundle.gain.close = failed_gain_close
            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: bundle,
                postprocessor=InlinePostprocessor(lambda request: None),
                coupling_gate=lambda point: None,
                sleep=lambda seconds: None,
                clock=incrementing_clock(),
                log=logging.getLogger("repeat-retained-session-test"),
            )

            with self.assertRaisesRegex(RuntimeError, "gain close failed") as caught:
                runner.run()

            report = getattr(caught.exception, "cleanup_report", None)
            self.assertIsNotNone(report)
            self.assertEqual(report.unreleased, (("gain", bundle.gain),))
            self.assertIsNotNone(runner._session)
            self.assertIs(runner._cleanup_reports[-1], report)

    def test_gain_fault_after_acquisition_stops_before_next_voltage(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)
            original_acquire = bundle.osa.acquire

            def acquire_then_fault(*args, **kwargs):
                spectrum = original_acquire(*args, **kwargs)
                bundle.gain.state = DriverState.FAULT
                return spectrum

            bundle.osa.acquire = acquire_then_fault
            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: bundle,
                postprocessor=InlinePostprocessor(lambda request: None),
                coupling_gate=lambda point: None,
                sleep=lambda seconds: None,
                clock=incrementing_clock(),
            )

            with self.assertRaises(DeviceFault):
                runner.run()

            first_acquire = events.index("osa.acquire.1")
            self.assertFalse(
                any(
                    event.startswith("voltage.set.")
                    for event in events[first_acquire + 1 :]
                )
            )
            status_path = next(store.path.glob("cycle_*/attempt_*/status.json"))
            status = json.loads(status_path.read_text(encoding="utf-8"))
            self.assertEqual(status["state"], "failed")
            self.assertEqual(status["acquisitions"], [])
            self.assertEqual(events[-6:], [
                "voltage.zero.emergency",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ])

    def test_holds_gain_outputs_across_cycles_and_acquires_exact_schedule(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=2, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            requests = []
            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: fake_bundle(events),
                postprocessor=InlinePostprocessor(lambda request: requests.append(request.cycle_index)),
                coupling_gate=lambda _point: events.append("coupled"),
                sleep=lambda _seconds: None,
                clock=incrementing_clock(),
                log=logging.getLogger("repeat-test"),
            )
            summary = runner.run()
            self.assertEqual(summary.acquisitions, 8)
            self.assertEqual(summary.completed_cycles, 2)
            self.assertEqual(requests, [0, 1])
            self.assertEqual(events.count("gain.enable_current"), 1)
            first = events.index("osa.acquire.1")
            last = events.index("osa.acquire.8")
            self.assertNotIn("gain.disable_current", events[first:last])
            self.assertEqual(events[-6:], [
                "voltage.zero.emergency",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ])

    def test_osa_failure_stops_later_commands_and_runs_full_cleanup(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=2, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            with self.assertRaisesRegex(RuntimeError, "OSA failed"):
                RepeatExperimentRunner(
                    config,
                    store,
                    lambda: fake_bundle(events, fail_at=5),
                    postprocessor=InlinePostprocessor(lambda _request: None),
                    coupling_gate=lambda _point: None,
                    sleep=lambda _seconds: None,
                    clock=incrementing_clock(),
                ).run()
            self.assertEqual(sum(event.startswith("osa.acquire.") for event in events), 5)
            self.assertEqual(events[-6:], [
                "voltage.zero.emergency",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ])
            status_files = tuple(store.path.glob("cycle_*/attempt_*/status.json"))
            states = [json.loads(path.read_text(encoding="utf-8"))["state"] for path in status_files]
            self.assertEqual(states, ["acquired", "failed"])

    def test_keyboard_interrupt_also_runs_cleanup(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            with self.assertRaises(KeyboardInterrupt):
                RepeatExperimentRunner(
                    config,
                    store,
                    lambda: fake_bundle(events, interrupt_at=2),
                    postprocessor=InlinePostprocessor(lambda _request: None),
                    coupling_gate=lambda _point: None,
                    sleep=lambda _seconds: None,
                    clock=incrementing_clock(),
                ).run()
            self.assertEqual(events[-6:], [
                "voltage.zero.emergency",
                "gain.disable_current",
                "gain.disable_tec",
                "gain.close",
                "voltage.close",
                "osa.close",
            ])

    def test_cleanup_failure_still_closes_later_hardware_and_postprocessor(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)

            def failed_gain_close():
                events.append("gain.close")
                raise RuntimeError("gain close failed")

            bundle.gain.close = failed_gain_close

            class TrackingPostprocessor(InlinePostprocessor):
                def close_and_drain(self):
                    events.append("postprocess.close")
                    return super().close_and_drain()

            quiet_log = logging.getLogger("repeat-cleanup-failure-test")
            quiet_log.disabled = True

            with self.assertRaisesRegex(RuntimeError, "gain close failed"):
                RepeatExperimentRunner(
                    config,
                    store,
                    lambda: bundle,
                    postprocessor=TrackingPostprocessor(lambda _request: None),
                    coupling_gate=lambda _point: None,
                    sleep=lambda _seconds: None,
                    clock=incrementing_clock(),
                    log=quiet_log,
                ).run()
            self.assertEqual(events[-3:], ["voltage.close", "osa.close", "postprocess.close"])

    def test_acquired_cycle_with_missing_products_is_postprocessed_without_instruments(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            events = []
            bundle = fake_bundle(events)
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            factory_calls = []

            def process(request):
                write_cycle_products(load_complete_cycle(request.attempt_path), config)

            summary = RepeatExperimentRunner(
                config,
                RepeatRunStore.open_existing(store.path),
                lambda: factory_calls.append("constructed"),
                postprocessor=InlinePostprocessor(process),
            ).run()
            self.assertEqual(factory_calls, [])
            self.assertEqual(summary.acquisitions, 0)
            self.assertEqual(summary.postprocessed_only_cycles, 1)
            self.assertTrue(attempt.path.joinpath("comparison.png").is_file())

    def test_acquired_status_is_requeued_even_when_cycle_products_exist(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            write_cycle_products(load_complete_cycle(attempt.path), config)
            requests = []

            summary = RepeatExperimentRunner(
                config,
                RepeatRunStore.open_existing(store.path),
                lambda: self.fail("real instruments must not be constructed"),
                postprocessor=InlinePostprocessor(
                    lambda request: requests.append(request.cycle_index)
                ),
            ).run()
            self.assertEqual(summary.postprocessed_only_cycles, 1)
            self.assertEqual(requests, [0])

    def test_missing_run_summary_requeues_all_completed_cycles(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=2, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            for cycle_index in range(2):
                attempt = store.start_cycle_attempt(cycle_index)
                for step in cycle_steps(store.schedule, cycle_index):
                    attempt.save_acquisition(
                        step=step,
                        spectrum=bundle.osa.acquire(),
                        voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                        gain_status=bundle.gain.read_status(),
                        osa_duration_s=1.0,
                    )
                attempt.mark_acquisition_complete()
                write_cycle_products(load_complete_cycle(attempt.path), config)
                attempt.mark_postprocessed()
            requests = []

            summary = RepeatExperimentRunner(
                config,
                RepeatRunStore.open_existing(store.path),
                lambda: self.fail("real instruments must not be constructed"),
                postprocessor=InlinePostprocessor(
                    lambda request: requests.append(request)
                ),
            ).run()
            self.assertEqual(summary.postprocessed_only_cycles, 2)
            self.assertEqual([request.cycle_index for request in requests], [0, 1])
            self.assertEqual(
                [getattr(request, "reset_aggregate", False) for request in requests],
                [True, False],
            )

    def test_cycle_product_validation_rejects_corrupt_summary_json(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            write_cycle_products(load_complete_cycle(attempt.path), config)
            attempt.path.joinpath("summary.json").write_text("not-json", encoding="utf-8")
            self.assertFalse(
                RepeatExperimentRunner._products_complete(
                    load_complete_cycle(attempt.path), config
                )
            )

    def test_cycle_product_validation_rejects_structurally_corrupt_summary(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            write_cycle_products(load_complete_cycle(attempt.path), config)
            summary_path = attempt.path / "summary.json"
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            for key in (
                "aggregation_definitions",
                "thresholds",
                "excluded_turning_points",
                "excluded_turning_point_voltage_v",
            ):
                summary.pop(key)
            summary["classification"].pop("scope")
            summary["summary_metrics"]["p95_signal_mae_db"] = "corrupt"
            summary["discrepancy_ranking"] = [{}]
            summary_path.write_text(json.dumps(summary), encoding="utf-8")
            self.assertFalse(
                RepeatExperimentRunner._products_complete(
                    load_complete_cycle(attempt.path), config
                )
            )

    def test_run_product_validation_rejects_junk_graphics_and_incomplete_schema(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            store.path.joinpath("repeat_summary.csv").write_text(
                "cycle_index\n0\n", encoding="utf-8"
            )
            for suffix in (".svg", ".pdf", ".png"):
                store.path.joinpath("repeat_summary" + suffix).write_text(
                    "junk", encoding="utf-8"
                )
            runner = RepeatExperimentRunner(
                config,
                store,
                lambda: self.fail("instruments must not be constructed"),
                postprocessor=InlinePostprocessor(lambda _request: None),
            )
            self.assertFalse(runner._run_products_complete({0}))

    def test_runner_revalidates_products_after_postprocessor_drain(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            write_cycle_products(load_complete_cycle(attempt.path), config)
            attempt.mark_postprocessed()

            with self.assertRaisesRegex(RuntimeError, "final result validation"):
                RepeatExperimentRunner(
                    config,
                    RepeatRunStore.open_existing(store.path),
                    lambda: self.fail("instruments must not be constructed"),
                    postprocessor=InlinePostprocessor(
                        lambda _request: None,
                        validate_products=True,
                    ),
                ).run()

    def test_resume_progress_and_eta_use_only_pending_acquisitions(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=2, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            acquire_cycle = fake_bundle([])
            first = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                first.save_acquisition(
                    step=step,
                    spectrum=acquire_cycle.osa.acquire(),
                    voltage_status=acquire_cycle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=acquire_cycle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            first.mark_acquisition_complete()
            stream = io.StringIO()
            logger = logging.getLogger(f"repeat-resume-progress-{id(stream)}")
            logger.setLevel(logging.INFO)
            logger.propagate = False
            handler = logging.StreamHandler(stream)
            logger.addHandler(handler)
            try:
                RepeatExperimentRunner(
                    config,
                    RepeatRunStore.open_existing(store.path),
                    lambda: fake_bundle([]),
                    postprocessor=InlinePostprocessor(lambda _request: None),
                    coupling_gate=lambda _point: None,
                    sleep=lambda _seconds: None,
                    clock=incrementing_clock(),
                    log=logger,
                ).run()
            finally:
                logger.removeHandler(handler)
            self.assertIn("completed=1/4", stream.getvalue())
            self.assertNotIn("completed=1/8", stream.getvalue())

    def test_resume_submission_failure_still_closes_postprocessor(self):
        with tempfile.TemporaryDirectory() as root:
            config = config_for(Path(root), cycles=1, points=2)
            store = RepeatRunStore.create(config, run_mode="real")
            bundle = fake_bundle([])
            attempt = store.start_cycle_attempt(0)
            for step in cycle_steps(store.schedule, 0):
                attempt.save_acquisition(
                    step=step,
                    spectrum=bundle.osa.acquire(),
                    voltage_status=bundle.voltage.wait_for_channel(1, step.voltage_v),
                    gain_status=bundle.gain.read_status(),
                    osa_duration_s=1.0,
                )
            attempt.mark_acquisition_complete()
            events = []

            class FailingSubmitPostprocessor:
                def submit(self, _request):
                    raise RuntimeError("submit failed")

                def check(self):
                    return None

                def close_and_drain(self):
                    events.append("postprocess.close")

            with self.assertRaisesRegex(RuntimeError, "submit failed"):
                RepeatExperimentRunner(
                    config,
                    RepeatRunStore.open_existing(store.path),
                    lambda: self.fail("real instruments must not be constructed"),
                    postprocessor=FailingSubmitPostprocessor(),
                ).run()
            self.assertEqual(events, ["postprocess.close"])


def incrementing_clock():
    values = iter(float(index) for index in range(10000))
    return lambda: next(values)


if __name__ == "__main__":
    unittest.main()
