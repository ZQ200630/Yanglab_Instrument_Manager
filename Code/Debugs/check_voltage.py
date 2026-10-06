"""Safe, operator-confirmed voltage-source diagnostic."""

from __future__ import annotations

import argparse
import math
import time

from Code.Utils import InstrumentSafetyError, VoltageSource, VoltageStatus


def _print_status(status: VoltageStatus) -> None:
    for channel, (voltage, current) in enumerate(
        zip(status.voltage_v, status.current_ma), start=1
    ):
        print(f"CH{channel}: {voltage:.4f} V, {current:.4f} mA")


def _run_check(
    *,
    port: str | None = None,
    source_factory=VoltageSource,
    input_fn=input,
    print_fn=print,
) -> None:
    """Run the state-changing portion with injectable boundaries for offline tests."""
    with source_factory(port=port) as source:
        print_fn("Verified startup zero; measured status:")
        _print_status_with(source.read_status(), print_fn)
        answer = input_fn("Apply 0.1 V to CH1 and then return to zero? [y/N]")
        if answer.strip().lower() != "y":
            return
        primary_error: BaseException | None = None
        try:
            source.set_channel(1, 0.1)
            print_fn("Fresh status after applying 0.1 V:")
            _print_status_with(source.wait_for_status(), print_fn)
        except BaseException as error:
            primary_error = error
            raise
        finally:
            try:
                source.zero(emergency=True)
            except BaseException:
                if primary_error is not None:
                    # Preserve the operation/interruption error; the driver records the
                    # cleanup failure and outer context cleanup still executes.
                    pass
                else:
                    raise


def _run_zero_monitor(
    *,
    duration_s: float,
    port: str | None = None,
    source_factory=VoltageSource,
    print_fn=print,
    clock=time.monotonic,
) -> int:
    """Monitor fresh telemetry while never commanding a nonzero voltage."""

    if (
        isinstance(duration_s, bool)
        or not isinstance(duration_s, (int, float))
        or not math.isfinite(float(duration_s))
        or float(duration_s) <= 0.0
    ):
        raise ValueError("duration_s must be finite and positive")

    duration = float(duration_s)
    samples = 0
    primary_error: BaseException | None = None
    with source_factory(port=port) as source:
        try:
            initial = source.read_status()
            _require_zero_status(initial)
            print_fn(
                "Verified startup zero; monitoring fresh telemetry for "
                f"{duration:.1f} s without commanding a nonzero output."
            )
            deadline = clock() + duration
            while True:
                remaining = deadline - clock()
                if remaining <= 0.0:
                    break
                # Use a complete telemetry interval even when only a tiny piece of
                # the requested soak duration remains.  Shrinking this timeout to
                # ``remaining`` creates a false failure at the deadline boundary.
                status = source.wait_for_status(timeout=1.0)
                _require_zero_status(status)
                samples += 1
            print_fn(f"0 V telemetry monitor complete: {samples} fresh frames checked.")
        except BaseException as error:
            primary_error = error
            raise
        finally:
            try:
                source.zero(emergency=True)
            except BaseException:
                if primary_error is None:
                    raise
    return samples


def _require_zero_status(status: VoltageStatus, tolerance_v: float = 0.05) -> None:
    if any(abs(value) >= tolerance_v for value in status.voltage_v):
        raise InstrumentSafetyError(
            "Voltage Source left the 0 V monitor tolerance; emergency zero requested"
        )


def _print_status_with(status: VoltageStatus, print_fn) -> None:
    for channel, (voltage, current) in enumerate(
        zip(status.voltage_v, status.current_ma), start=1
    ):
        print_fn(f"CH{channel}: {voltage:.4f} V, {current:.4f} mA")


def main() -> int:
    parser = argparse.ArgumentParser(description="Check an eight-channel voltage source safely.")
    parser.add_argument("--port", help="serial port (otherwise auto-discover the CH340)")
    parser.add_argument(
        "--monitor-zero-seconds",
        type=float,
        help=(
            "monitor fresh telemetry for this duration while commanding only 0 V; "
            "skips the interactive 0.1 V diagnostic"
        ),
    )
    args = parser.parse_args()

    if args.monitor_zero_seconds is not None:
        _run_zero_monitor(duration_s=args.monitor_zero_seconds, port=args.port)
    else:
        _run_check(port=args.port)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
