"""Staged, preservation-first PM400 diagnostic."""

from __future__ import annotations

import argparse
import math
import sys
from collections.abc import Sequence

import pyvisa

from Code.Utils.pm400 import PM400


def _positive_seconds(text: str) -> float:
    try:
        value = float(text)
    except (TypeError, ValueError) as error:
        raise argparse.ArgumentTypeError("timeout must be finite and positive") from error
    if not math.isfinite(value) or value <= 0:
        raise argparse.ArgumentTypeError("timeout must be finite and positive")
    return value


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Enumerate or perform a read-only Thorlabs PM400 check."
    )
    parser.add_argument("--resource", help="operator-selected PM400 VISA resource")
    parser.add_argument("--timeout", type=_positive_seconds, default=5.0, metavar="SECONDS")
    parser.add_argument(
        "--no-op-check",
        action="store_true",
        help="offer a separately confirmed same-brightness write",
    )
    return parser


def _attach(error: BaseException, name: str, secondary: BaseException) -> None:
    try:
        setattr(error, name, secondary)
    except BaseException:
        try:
            error.add_note(f"{name}: {type(secondary).__name__}: {secondary}")
        except BaseException:
            pass


def _list_resources() -> int:
    manager = pyvisa.ResourceManager()
    try:
        resources = manager.list_resources()
        print("VISA resources:")
        if not resources:
            print("  (none)")
        for resource in resources:
            print(f"  {resource}")
        return 0
    finally:
        try:
            manager.close()
        except BaseException as cleanup_error:
            primary = sys.exception()
            if primary is None:
                raise
            _attach(primary, "pm400_listing_cleanup_error", cleanup_error)


def _print_connected(driver: PM400, timeout: float, no_op_check: bool) -> int:
    info = driver.instrument_info
    sensor = driver.sensor_info
    if info is None or sensor is None:
        raise RuntimeError("PM400 connected without identity and sensor information")

    print(
        "Instrument: "
        f"manufacturer={info.manufacturer}, model={info.model}, "
        f"serial={info.serial_number}, firmware={info.firmware}"
    )
    capabilities = sensor.capabilities
    print(
        "Sensor: "
        f"name={sensor.name}, serial={sensor.serial_number}, "
        f"calibration={sensor.calibration_message}, type={sensor.sensor_type}, "
        f"subtype={sensor.subtype}, flags={sensor.raw_flags}"
    )
    print(
        "Capabilities: "
        f"power={capabilities.power}, energy={capabilities.energy}, "
        f"response_settable={capabilities.response_settable}, "
        f"wavelength_settable={capabilities.wavelength_settable}, "
        f"tau_settable={capabilities.tau_settable}, "
        f"temperature_sensor={capabilities.temperature_sensor}"
    )

    configuration = driver.measurement.get_configuration()
    print(f"Current measurement configuration: {configuration.name}")
    measurement = driver.measurement.read(timeout=timeout)
    print(
        f"Measurement: kind={measurement.kind.name}, value={measurement.value:g} "
        f"{measurement.unit}, monotonic_time={measurement.measured_at:g}"
    )

    if not no_op_check:
        return 0

    brightness = driver.display.get_brightness()
    print(
        "Optional reversible no-op: current brightness is "
        f"{brightness:g}; proposed setter is set_brightness({brightness:g})."
    )
    print("Type exactly WRITE SAME VALUE to authorize this one same-value write.")
    try:
        response = input("Confirmation: ")
    except (EOFError, KeyboardInterrupt, SystemExit):
        print("No-op write declined; no setter was called.")
        return 0
    if response != "WRITE SAME VALUE":
        print("No-op write declined; no setter was called.")
        return 0
    confirmed = driver.display.set_brightness(brightness)
    print(f"Same-value brightness readback: {confirmed:g}")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.resource is None:
        return _list_resources()

    driver = PM400(args.resource, timeout=args.timeout)
    primary: BaseException | None = None
    traceback = None
    result = 0
    try:
        driver.connect()
        result = _print_connected(driver, args.timeout, args.no_op_check)
    except BaseException as error:
        primary = error
        traceback = error.__traceback__
    try:
        driver.close()
    except BaseException as cleanup_error:
        if primary is None:
            primary = cleanup_error
            traceback = cleanup_error.__traceback__
        else:
            _attach(primary, "pm400_diagnostic_close_error", cleanup_error)
    if primary is not None:
        raise primary.with_traceback(traceback)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
