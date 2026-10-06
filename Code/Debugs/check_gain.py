"""Safe, operator-confirmed Gain Chip Driver diagnostic."""

from __future__ import annotations

import argparse
import math
import time

from Code.Utils import GainDriver, GainStatus, InstrumentProtocolError, InstrumentSafetyError


MAX_HOLD_SECONDS = 60.0


def _print_status(status: GainStatus, pid: tuple[float, float, float], print_fn) -> None:
    print_fn(f"Temperature: {status.temperature_c:.3f} degC")
    print_fn(f"Target: {status.target_c:.3f} degC")
    print_fn(f"TEC enabled: {status.tec_enabled}")
    print_fn(f"Current setpoint: {status.current_ma:.3f} mA")
    print_fn(f"Current enabled: {status.current_enabled}")
    print_fn(f"PID: P={pid[0]:.3f}, I={pid[1]:.3f}, D={pid[2]:.3f}")


def _read_bounded_number(
    prompt: str,
    *,
    minimum: float,
    maximum: float,
    label: str,
    input_fn,
) -> float:
    try:
        value = float(input_fn(prompt).strip())
    except (AttributeError, TypeError, ValueError) as error:
        raise InstrumentSafetyError(f"{label} must be a number") from error
    if not math.isfinite(value) or not minimum <= value <= maximum:
        raise InstrumentSafetyError(f"{label} must be within {minimum}-{maximum}")
    return value


def _same_protocol_value(left: float, right: float) -> bool:
    """Compare values at a margin tighter than the three-decimal device field."""
    return math.isclose(left, right, rel_tol=0.0, abs_tol=0.0005)


def _require_noop_match(label: str, expected: float, actual: float) -> None:
    if not _same_protocol_value(expected, actual):
        raise InstrumentProtocolError(
            f"Gain diagnostic {label} no-op verification mismatch: {actual!r} != {expected!r}"
        )


def _run_check(
    *,
    port: str | None = None,
    serial_number: str | None = None,
    driver_factory=GainDriver,
    input_fn=input,
    print_fn=print,
    sleep_fn=time.sleep,
    status_only: bool = False,
) -> None:
    """Run read/no-op checks, with output changes gated by explicit consent."""
    factory_kwargs = {"port": port}
    if serial_number is not None:
        factory_kwargs["usb_serial"] = serial_number
    if status_only:
        # Opening may trigger safety shutdown; normal close always sends Q=0
        # before D=0. This is not a read-only lifecycle or a physical-off proof.
        driver = driver_factory(**factory_kwargs)
        with driver:
            status = driver.read_status()  # five typed connection readbacks
            print_fn(f"Temperature: {status.temperature_c:.3f} degC")
            print_fn(f"Target: {status.target_c:.3f} degC")
            print_fn(f"TEC enabled: {status.tec_enabled}")
            print_fn(f"Current setpoint: {status.current_ma:.3f} mA")
            print_fn(f"Current enabled: {status.current_enabled}")
        print_fn(f"close_returned=True; driver_state={driver.state.name}")
        return
    with driver_factory(**factory_kwargs) as driver:
        # A successful driver request has already parsed exact ASCII CRLF frames;
        # do not bypass that boundary by reading the transport here.
        status = driver.read_status()
        pid = driver.read_pid()
        print_fn("Strict CRLF framing verified by GainDriver reads.")
        _print_status(status, pid, print_fn)

        # Re-apply raw-confirmed values only.  RST and CLR are intentionally
        # excluded because they alter controller state rather than verify it.
        applied_target = driver.set_temperature(status.target_c)
        applied_current = driver.set_current(status.current_ma)
        applied_pid = driver.set_pid(*pid)
        _require_noop_match("target", status.target_c, applied_target)
        _require_noop_match("current", status.current_ma, applied_current)
        if len(applied_pid) != 3:
            raise InstrumentProtocolError("Gain diagnostic PID setter did not return three values")
        for label, expected, actual in zip(("P", "I", "D"), pid, applied_pid, strict=True):
            _require_noop_match(f"PID {label}", expected, actual)

        fresh_target = driver.read_target()
        fresh_current = driver.read_current()
        fresh_tec = driver.read_tec_enabled()
        fresh_current_enabled = driver.read_current_enabled()
        fresh_temperature = driver.read_temperature()
        fresh_pid = driver.read_pid()
        _require_noop_match("fresh target", status.target_c, fresh_target)
        _require_noop_match("fresh current", status.current_ma, fresh_current)
        try:
            fresh_temperature = float(fresh_temperature)
        except (TypeError, ValueError) as error:
            raise InstrumentProtocolError("Gain diagnostic fresh temperature is not numeric") from error
        if not math.isfinite(fresh_temperature):
            raise InstrumentProtocolError("Gain diagnostic fresh temperature is not finite")
        if fresh_tec != status.tec_enabled or fresh_current_enabled != status.current_enabled:
            raise InstrumentProtocolError("Gain diagnostic output state changed during no-op verification")
        for label, expected, actual in zip(("fresh PID P", "fresh PID I", "fresh PID D"), pid, fresh_pid, strict=True):
            _require_noop_match(label, expected, actual)
        print_fn(f"Fresh temperature: {fresh_temperature:.3f} degC")
        print_fn("No-op target/current/PID writes confirmed.")

        answer = input_fn(
            "Enable TEC/current for an output test? This can change laser drive state. [y/N] "
        )
        if answer.strip().lower() != "y":
            return

        current_ma = _read_bounded_number(
            "Test current in mA (0-200): ",
            minimum=0.0,
            maximum=200.0,
            label="test current",
            input_fn=input_fn,
        )
        hold_seconds = _read_bounded_number(
            f"Hold duration in seconds (0-{MAX_HOLD_SECONDS:g}): ",
            minimum=0.0,
            maximum=MAX_HOLD_SECONDS,
            label="hold duration",
            input_fn=input_fn,
        )

        driver.enable_tec()
        driver.wait_stable()
        driver.set_current(current_ma)
        primary_error: BaseException | None = None
        try:
            driver.enable_current()
            print_fn(f"Holding {current_ma:.3f} mA for {hold_seconds:.3f} s.")
            sleep_fn(hold_seconds)
        except BaseException as error:
            primary_error = error
            raise
        finally:
            try:
                driver.disable_current()
            except BaseException:
                if primary_error is None:
                    raise


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check a Gain Chip Driver with safe read/no-op and optional output tests."
    )
    parser.add_argument("--port", help="serial port (otherwise auto-discover CP210x)")
    parser.add_argument("--serial-number", help="preferred CP210x USB serial number")
    parser.add_argument("--status-only", action="store_true",
        help="read connection status only; no setting/PID writes or output enable. "
             "Opening can reset serial lines; close disables current before TEC.")
    args = parser.parse_args()
    _run_check(port=args.port, serial_number=args.serial_number, status_only=args.status_only)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
