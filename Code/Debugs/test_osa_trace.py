"""Native OSA trace data contracts; no instrument resources are constructed."""

import importlib
import unittest
from dataclasses import FrozenInstanceError, replace
from datetime import datetime, timedelta, timezone

import numpy as np

from Code.Utils.common import InstrumentProtocolError


class NativeTraceTests(unittest.TestCase):
    def api(self):
        try:
            return importlib.import_module("Code.Utils.osa_trace")
        except ModuleNotFoundError as error:
            if error.name != "Code.Utils.osa_trace":
                raise
            self.fail("Native OSA trace data API is not implemented")

    def context(self, **changes):
        values = dict(transfer_format="ASCII", sample_count=2, spacing=0,
                      level_unit=0, x_unit=0, trace_attribute=0,
                      active_trace="TRA", center_m=1.55e-6, span_m=1e-9,
                      resolution_m=2e-11, sweep_mode=2)
        values.update(changes)
        return self.api().TraceContext(**values)

    def capture(self, **changes):
        start = datetime(2026, 10, 6, tzinfo=timezone.utc)
        context = self.context()
        values = dict(wavelength_nm=[1550., 1551.], native_values=[-40., -30.],
                      native_unit="dBm", trace="A", identity="YOKOGAWA,AQ6370E,SN,FW",
                      read_started_at=start, read_finished_at=start + timedelta(seconds=1),
                      elapsed_s=1., context_before=context, context_after=context)
        values.update(changes)
        return self.api().TraceCapture(**values)

    def test_literal_ascii_and_little_endian_binary_samples(self):
        # Catches wrong byte order, width, scale, and sample parsing.
        replies = (("ASCII", b"-40,-30\r\n"),
                   ("REAL,32", b"#18\x00\x00\x20\xc2\x00\x00\xf0\xc1\n"),
                   ("REAL,64", b"#216\x00\x00\x00\x00\x00\x00\x44\xc0"
                    b"\x00\x00\x00\x00\x00\x00\x3e\xc0\r\n"))
        for fmt, reply in replies:
            with self.subTest(fmt=fmt):
                values = self.api().decode_trace_reply(reply, fmt, 2)
                np.testing.assert_array_equal(values, [-40., -30.])
                with self.assertRaises(ValueError):
                    values.setflags(write=True)

    def test_ascii_malformed_or_nonfinite_samples_are_not_partial_results(self):
        for reply in (b"", b"-40,", b"-40,, -30", b"nan,-30", b"inf,-30",
                      b"-40,-30garbage", b"-40,-30\nX", b"-40,-30,0",
                      b"-40,\xff", b"-40\n,-30"):
            with self.subTest(reply=reply):
                with self.assertRaises(InstrumentProtocolError):
                    self.api().decode_trace_reply(reply, "ASCII", 2)

    def test_binary_bad_headers_lengths_and_suffixes_fail_closed(self):
        for reply in (b"#0payload", b"#", b"#x8", b"#18abc", b"#28abcd",
                      b"#1999999999", b"#9167772160", b"#1-8", b"#1a",
                      b"#18\x00\x00\x20\xc2\x00\x00\xf0\xc1extra"):
            with self.subTest(reply=reply):
                with self.assertRaises(InstrumentProtocolError):
                    self.api().decode_trace_reply(reply, "REAL,32", 2)

    def test_binary_nonfinite_payload_is_rejected(self):
        with self.assertRaises(InstrumentProtocolError):
            self.api().decode_trace_reply(b"#18\x00\x00\xc0\x7f\x00\x00\xf0\xc1",
                                         "REAL,32", 2)

    def test_declared_limits_apply_before_parsing(self):
        cases = ((b"-40,-30", "ASCII", 0), (b"-40,-30", "ASCII", True),
                 (b"-40,-30", "ASCII", 1025), (b"-40,-30", "REAL,16", 2),
                 (b"1" * 65537, "ASCII", 2), ("-40,-30", "ASCII", 2))
        for reply, fmt, count in cases:
            with self.subTest(fmt=fmt, count=count):
                with self.assertRaises(InstrumentProtocolError):
                    self.api().decode_trace_reply(reply, fmt, count)

    def test_valid_suffix_and_chunk_ceiling_are_supported(self):
        for suffix in (b"", b"\n", b"\r\n"):
            with self.subTest(suffix=suffix):
                np.testing.assert_array_equal(
                    self.api().decode_trace_reply(b"-40,-30" + suffix, "ASCII", 2),
                    [-40., -30.])
        values = self.api().decode_trace_reply(b",".join([b"1"] * 1024), "ASCII", 1024)
        self.assertEqual(values.shape, (1024,))
        self.assertTrue(np.all(values == 1.))

    def test_context_validates_finite_bounds_and_keeps_zero_span(self):
        self.assertEqual(self.context(span_m=0.).span_m, 0.)
        self.assertEqual(self.context(sample_count=200001).sample_count, 200001)
        for changes in (dict(sample_count=0), dict(sample_count=200002),
                        dict(sample_count=True), dict(spacing=2), dict(level_unit=4),
                        dict(x_unit=-1), dict(trace_attribute=6), dict(sweep_mode=0),
                        dict(center_m=0), dict(span_m=-1), dict(resolution_m=0),
                        dict(center_m=float("nan")), dict(span_m=float("inf")),
                        dict(active_trace="TRH"), dict(transfer_format="REAL,16")):
            with self.subTest(changes=changes):
                with self.assertRaises(InstrumentProtocolError):
                    self.context(**changes)

    def test_density_math_and_contradictory_scale_have_no_absolute_unit(self):
        for changes in (dict(level_unit=2), dict(level_unit=3, spacing=1),
                        dict(trace_attribute=5), dict(level_unit=1, spacing=0),
                        dict(level_unit=0, spacing=1)):
            with self.subTest(changes=changes):
                with self.assertRaises(InstrumentProtocolError):
                    _ = self.context(**changes).native_unit
        self.assertEqual(self.context().native_unit, "dBm")
        self.assertEqual(self.context(spacing=1, level_unit=1).native_unit, "W")

    def test_frequency_context_cannot_label_native_hz_values_as_metres(self):
        with self.assertRaises(InstrumentProtocolError):
            self.context(x_unit=1, center_m=193414489032258.06,
                         span_m=250000000000., resolution_m=2500000000.)

    def test_capture_owns_immutable_native_samples(self):
        wavelength, levels = np.array([1550., 1551.]), np.array([-40., -30.])
        capture = self.capture(wavelength_nm=wavelength, native_values=levels)
        wavelength[0], levels[0] = 0., 0.
        np.testing.assert_array_equal(capture.wavelength_nm, [1550., 1551.])
        np.testing.assert_array_equal(capture.native_values, [-40., -30.])
        np.testing.assert_array_equal(capture.power_dbm, [-40., -30.])
        for values in (capture.wavelength_nm, capture.native_values, capture.power_dbm):
            with self.assertRaises(ValueError):
                values.setflags(write=True)
        with self.assertRaises(FrozenInstanceError):
            capture.native_unit = "W"
        with self.assertRaises(FrozenInstanceError):
            capture.context_before.spacing = 1

    def test_watt_conversion_preserves_native_samples_and_finite_semantics(self):
        context = self.context(level_unit=1, spacing=1)
        capture = self.capture(native_values=[.001, .01], native_unit="W",
                               context_before=context, context_after=context)
        np.testing.assert_array_equal(capture.native_values, [.001, .01])
        np.testing.assert_allclose(capture.power_dbm, [0., 10.], rtol=0, atol=1e-12)
        with self.assertRaises(ValueError):
            capture.power_dbm.setflags(write=True)
        zero = replace(capture, native_values=[0., .01])
        self.assertIsNone(zero.power_dbm)

    def test_negative_absolute_power_cannot_claim_a_valid_capture(self):
        context = self.context(level_unit=1, spacing=1)
        with self.assertRaises(InstrumentProtocolError):
            self.capture(native_values=[-.001, .01], native_unit="W",
                         context_before=context, context_after=context)

    def test_capture_rejects_wrong_shape_count_finiteness_or_axis(self):
        for changes in (dict(wavelength_nm=[]), dict(native_values=[-40.]),
                        dict(native_values=[[-40., -30.]]),
                        dict(native_values=[-40., float("nan")]),
                        dict(wavelength_nm=[1551., 1550.]),
                        dict(wavelength_nm=[1550., 1550.]),
                        dict(wavelength_nm=[0., 1551.]),
                        dict(wavelength_nm=[float("inf"), 1551.]),
                        dict(native_values=["garbage", -30.])):
            with self.subTest(changes=changes):
                with self.assertRaises(InstrumentProtocolError):
                    self.capture(**changes)

    def test_changed_context_or_wrong_unit_never_publishes_a_complete_capture(self):
        for changes in (dict(context_after=self.context(sample_count=3)),
                        dict(context_after=self.context(transfer_format="REAL,32")),
                        dict(context_after=self.context(center_m=1.56e-6)),
                        dict(native_unit="W"), dict(consistency="atomic"),
                        dict(trace="H"), dict(identity="")):
            with self.subTest(changes=changes):
                with self.assertRaises(InstrumentProtocolError):
                    self.capture(**changes)

    def test_read_timestamps_do_not_claim_instrument_measurement_time(self):
        capture = self.capture()
        self.assertEqual(capture.consistency, "unproven")
        self.assertFalse(hasattr(capture, "measurement_time"))
        # A wall-clock adjustment must not corrupt the monotonic elapsed value.
        adjusted = replace(capture, read_finished_at=capture.read_started_at - timedelta(seconds=1))
        self.assertEqual(adjusted.elapsed_s, 1.)
        for changes in (dict(elapsed_s=-1), dict(elapsed_s=float("nan")),
                        dict(read_started_at=datetime(2026, 10, 6))):
            with self.subTest(changes=changes):
                with self.assertRaises(InstrumentProtocolError):
                    self.capture(**changes)


if __name__ == "__main__":
    unittest.main()
