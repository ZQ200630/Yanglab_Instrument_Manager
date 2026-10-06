"""Status-only diagnostic uses the actual Gain driver and a bounded serial wire."""
import io
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

from Code.Debugs import check_gain
from Code.Debugs.test_drivers import FakeGainSerial
from Code.Utils.gain import GainDriver
from Code.Utils.common import InstrumentProtocolError


STATUS = (
    b"READY;T=21.456\r\n", b"READY;E=22.000\r\n",
    b"READY;R=0\r\n", b"READY;C=150.000\r\n", b"READY;Q=0\r\n",
)


class GainStatusDiagnosticTests(unittest.TestCase):
    def _run(self, wire):
        driver = GainDriver(port="COM995", start_watchdog=False,
                            serial_factory=lambda **_: wire)
        output = []
        try:
            try:
                check_gain._run_check(port="COM995", status_only=True,
                    driver_factory=lambda **_: driver,
                    input_fn=lambda _: self.fail("Status-only must never prompt to enable outputs"),
                    print_fn=output.append)
            except TypeError as error:
                self.fail(f"Status-only diagnostic routing is missing: {error}")
            return driver, output
        finally:
            driver.close()

    def test_status_only_does_not_rewrite_settings_and_closes_current_before_tec(self):
        wire = FakeGainSerial(STATUS)
        driver, output = self._run(wire)
        self.assertEqual(wire.writes, [
            b"RDTA\r\n", b"RDEA\r\n", b"RDRA\r\n", b"RDCA\r\n", b"RDQA\r\n",
            b"STQA000000\r\n", b"STRA000000\r\n",
        ])
        self.assertFalse(wire.is_open)
        self.assertIn("Current setpoint: 150.000 mA", output)
        self.assertIn("close_returned=True; driver_state=DISCONNECTED", output)

    def test_status_failure_still_attempts_ordered_shutdown_and_resource_release(self):
        wire = FakeGainSerial((b"UNEXPECTED\r\n",))
        with self.assertRaises(InstrumentProtocolError):
            self._run(wire)
        self.assertEqual(wire.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
        self.assertFalse(wire.is_open)

    def test_cli_status_only_cannot_fall_through_to_default_noop_writes(self):
        wire = FakeGainSerial(STATUS)
        driver = GainDriver(port="COM995", start_watchdog=False,
                            serial_factory=lambda **_: wire)
        original = check_gain._run_check
        try:
            with patch("sys.argv", ["check_gain", "--status-only", "--port", "COM995"]), \
                 patch.object(check_gain, "_run_check", wraps=check_gain._run_check) as route, \
                 patch("builtins.input", side_effect=AssertionError("No output prompt")):
                # Factory injection is explicit and only at the driver boundary.
                def finite_route(**kwargs):
                    return original(**kwargs, driver_factory=lambda **_: driver)
                route.side_effect = finite_route
                with redirect_stdout(io.StringIO()):
                    try:
                        self.assertEqual(check_gain.main(), 0)
                    except SystemExit as error:
                        self.fail(f"Status-only CLI is missing: exit {error.code}")
            self.assertEqual(wire.writes[-2:], [b"STQA000000\r\n", b"STRA000000\r\n"])
            self.assertFalse(wire.is_open)
        finally:
            driver.close()


if __name__ == "__main__":
    unittest.main()
