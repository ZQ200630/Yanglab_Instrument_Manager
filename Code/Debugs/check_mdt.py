"""Staged, preservation-first MDT693B diagnostic."""

from __future__ import annotations

import argparse
import math
from collections.abc import Mapping, Sequence

from serial.tools import list_ports

from Code.Utils.common import DeviceFault, InstrumentSafetyError
from Code.Utils.mdt693b import Axis, AxisState, MDT693B, MDTStatus


_MASTER_ZERO_TOLERANCE_V = 1e-6


def _step_voltage(text: str) -> float:
    try:
        value = float(text)
    except (TypeError, ValueError) as error:
        raise argparse.ArgumentTypeError("step voltage must be within (0, 0.1] V") from error
    if not math.isfinite(value) or value <= 0 or value > 0.1:
        raise argparse.ArgumentTypeError("step voltage must be within (0, 0.1] V")
    return value


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Enumerate or perform a read-only Thorlabs MDT693B check."
    )
    parser.add_argument("--port", help="operator-selected MDT693B serial port")
    parser.add_argument("--step-axis", choices=("X", "Y", "Z"))
    parser.add_argument("--step-volts", type=_step_voltage, default=0.1, metavar="VOLTS")
    return parser


def _attach(error: BaseException, name: str, secondary: BaseException) -> None:
    try:
        setattr(error, name, secondary)
    except BaseException:
        try:
            error.add_note(f"{name}: {type(secondary).__name__}: {secondary}")
        except BaseException:
            pass


def _list_serial_ports() -> int:
    ports = list_ports.comports()
    print("Serial ports:")
    if not ports:
        print("  (none)")
    for port in ports:
        vid = "----" if port.vid is None else f"{port.vid:04X}"
        pid = "----" if port.pid is None else f"{port.pid:04X}"
        serial_number = port.serial_number or ""
        description = port.description or ""
        print(
            f"  device={port.device}, VID={vid}, PID={pid}, "
            f"serial={serial_number}, description={description}"
        )
    return 0


def _print_status(status: MDTStatus) -> None:
    commands = ", ".join(sorted(status.supported_commands))
    print(f"Supported commands: {commands}")
    print(f"xmin? advertised: {'xmin?' in status.supported_commands}")
    print(
        f"Identity: product={status.product}, firmware={status.firmware}, "
        f"serial={status.serial_number}, friendly_name={status.friendly_name}"
    )
    print(
        f"Hardware limit: {status.hardware_limit.volts:g} V; "
        f"echo={status.echo_enabled}; display_intensity={status.display_intensity}"
    )
    print(
        f"Master Scan: enabled={status.master_scan_enabled}, "
        f"voltage={status.master_scan_voltage_v:g} V"
    )
    for axis in Axis:
        state = status.axes[axis]
        print(
            f"{axis.name}: actual={state.actual_v:g} V, "
            f"minimum={state.minimum_v:g} V, maximum={state.maximum_v:g} V"
        )
    print(
        f"DAC step={status.dac_step}; compatibility={status.compatibility_enabled}; "
        f"rotary={status.rotary_mode.name}; "
        f"push_to_adjust_disabled={status.push_to_adjust_disabled}"
    )
    print(
        f"restricted={status.restricted}; fault={status.fault_evidence}; "
        f"axis_command_known={status.axis_command_known}; "
        f"observed_at={status.observed_at}"
    )


def _print_axes(label: str, actual: Mapping[Axis, float]) -> None:
    values = ", ".join(f"{axis.name}={actual[axis]:g} V" for axis in Axis)
    print(f"{label}: {values}")


def _motion_target(
    driver: MDT693B, status: MDTStatus, axis: Axis, state: AxisState, step_v: float
) -> float:
    lower = max(0.0, state.minimum_v)
    upper = min(
        75.0,
        status.hardware_limit.volts,
        state.maximum_v,
        driver.application_limits_v[axis],
    )
    old = state.actual_v
    if old < lower or old > upper:
        raise ValueError("selected axis is outside the reversible diagnostic limits")
    upward = old + step_v
    if upward <= upper:
        return upward
    downward = old - step_v
    if downward >= lower:
        return downward
    raise ValueError("no reversible diagnostic target fits the selected axis limits")


def _require_zero_master_scan(driver: MDT693B) -> None:
    enabled = driver.get_master_scan_enabled()
    voltage = driver.get_master_scan_voltage()
    if enabled is True or abs(voltage) > _MASTER_ZERO_TOLERANCE_V:
        raise InstrumentSafetyError(
            "Reversible axis diagnostic requires freshly confirmed disabled "
            "Master Scan with zero voltage contribution"
        )


def _run_connected(driver: MDT693B, step_axis: str | None, step_v: float) -> int:
    driver.connect()
    status = driver.status
    if status is None:
        raise RuntimeError("MDT693B connected without a status snapshot")
    _print_status(status)
    if step_axis is None:
        return 0
    if not status.axis_command_known:
        print(
            "Absolute axis motion is disarmed because the base-axis command "
            "is unknown; no target was calculated and no prompt or setter was used."
        )
        return 0

    axis = Axis[step_axis]
    _require_zero_master_scan(driver)
    fresh = driver.get_axis_state(axis)
    old = fresh.actual_v
    target = _motion_target(driver, status, axis, fresh, step_v)
    print(
        f"Proposed reversible move: axis={axis.name}, old={old:g} V, "
        f"target={target:g} V, restoration={old:g} V."
    )
    print("Type exactly MOVE to authorize the move and exact-old restoration.")
    try:
        response = input("Confirmation: ")
    except (EOFError, KeyboardInterrupt, SystemExit):
        print("Move declined; no voltage setter was called.")
        return 0
    if response != "MOVE":
        print("Move declined; no voltage setter was called.")
        return 0

    _require_zero_master_scan(driver)
    confirmed = driver.get_axis_state(axis)
    live_status = driver.status
    if live_status is None or not live_status.axis_command_known:
        raise InstrumentSafetyError(
            "Axis-command authority changed during diagnostic confirmation"
        )
    if not math.isclose(
        confirmed.actual_v,
        old,
        rel_tol=0.0,
        abs_tol=_MASTER_ZERO_TOLERANCE_V,
    ):
        raise InstrumentSafetyError(
            f"Selected-axis actual changed during confirmation: old={old:g} V, "
            f"observed={confirmed.actual_v:g} V"
        )

    primary: BaseException | None = None
    traceback = None
    motion_may_have_been_issued = False
    try:
        motion_may_have_been_issued = True
        driver.set_axis_voltage(axis, target)
        _print_axes("Observed after move", driver.get_all_voltages())
    except BaseException as error:
        primary = error
        traceback = error.__traceback__
    finally:
        if motion_may_have_been_issued:
            try:
                driver.set_axis_voltage(axis, old)
                restored_actual = driver.get_all_voltages()
                observed = restored_actual[axis]
                if not math.isclose(
                    observed, old, rel_tol=0.0, abs_tol=_MASTER_ZERO_TOLERANCE_V
                ):
                    raise DeviceFault(
                        f"MDT restoration mismatch for {axis.name}: "
                        f"old={old!r} V, observed={observed!r} V, "
                        f"abs_tolerance={_MASTER_ZERO_TOLERANCE_V!r} V"
                    )
            except BaseException as restoration_error:
                if primary is None:
                    primary = restoration_error
                    traceback = restoration_error.__traceback__
                else:
                    _attach(primary, "mdt_diagnostic_restore_error", restoration_error)
            else:
                _print_axes("Observed after restoration", restored_actual)
                print(f"Restored {axis.name} to the exact old value {old:g} V.")
    if primary is not None:
        raise primary.with_traceback(traceback)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    if args.step_axis is not None and args.port is None:
        parser.error("--step-axis requires --port")
    if args.port is None:
        return _list_serial_ports()

    driver = MDT693B(args.port)
    primary: BaseException | None = None
    traceback = None
    result = 0
    try:
        result = _run_connected(driver, args.step_axis, args.step_volts)
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
            _attach(primary, "mdt_diagnostic_close_error", cleanup_error)
    if primary is not None:
        raise primary.with_traceback(traceback)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
