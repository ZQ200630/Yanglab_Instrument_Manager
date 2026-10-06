"""Explicitly staged diagnostic for the registered fiber-coupling setup.

This program deliberately exposes only setup-level logical coordinates.  It
does not enumerate, connect, adopt a baseline, or request motion at import.
"""

from __future__ import annotations

import argparse
import math
from collections.abc import Callable, Sequence

from Code.Setups import FiberCouplingSetup, LogicalAxis, StageSide, StageStatus, Vector3Um


def _nonzero_finite_um(text: str) -> float:
    try:
        value = float(text)
    except (TypeError, ValueError) as error:
        raise argparse.ArgumentTypeError("--um must be a finite, nonzero displacement") from error
    if not math.isfinite(value) or value == 0.0:
        raise argparse.ArgumentTypeError("--um must be a finite, nonzero displacement")
    return value


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Staged read-only and explicitly confirmed fiber-stage diagnostic."
    )
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--enumerate", action="store_true", help="list registered stage identities only")
    modes.add_argument("--read-only", action="store_true", help="connect and display logical read-only status")
    modes.add_argument("--move", action="store_true", help="perform one separately confirmed relative move")
    parser.add_argument("--side", choices=tuple(side.value for side in StageSide))
    parser.add_argument("--axis", choices=tuple(axis.value for axis in LogicalAxis))
    parser.add_argument("--um", type=_nonzero_finite_um, metavar="UM")
    parser.add_argument("--allow-nominal", action="store_true")
    parser.add_argument("--config", metavar="PATH")
    return parser


def _setup_kwargs(config_path: str | None) -> dict[str, str]:
    return {} if config_path is None else {"config_path": config_path}


def _attach(primary: BaseException, name: str, secondary: BaseException) -> None:
    try:
        setattr(primary, name, secondary)
    except BaseException:
        try:
            primary.add_note(f"{name}: {type(secondary).__name__}: {secondary}")
        except BaseException:
            pass


def _run_connected(config_path: str | None, action: Callable[[object], int]) -> int:
    """Run an entire connected operation with body-error cleanup precedence."""
    setup = FiberCouplingSetup.connect(**_setup_kwargs(config_path))
    primary: BaseException | None = None
    traceback = None
    result = 1
    try:
        result = action(setup)
    except BaseException as error:
        primary = error
        traceback = error.__traceback__
    try:
        setup.close()
    except BaseException as cleanup_error:
        if primary is None:
            if result != 0:
                print(f"Close also failed: {type(cleanup_error).__name__}: {cleanup_error}")
                return result
            primary = cleanup_error
            traceback = cleanup_error.__traceback__
        else:
            _attach(primary, "fiber_diagnostic_close_error", cleanup_error)
    if primary is not None:
        raise primary.with_traceback(traceback)
    return result


def _print_discovery(discovery: object) -> None:
    print("Registered fiber-stage discovery:")
    for side in StageSide:
        info = discovery.registered.get(side)
        if info is None:
            print(f"  {side.name}: missing")
        else:
            print(f"  {side.name}: serial={info.serial_number}, resource={info.resource}, description={info.description}")
    if discovery.unknown_devices:
        print("Unknown candidate devices:")
        for info in discovery.unknown_devices:
            print(f"  serial={info.serial_number}, resource={info.resource}, description={info.description}")


def _sources(status: StageStatus) -> str:
    return ", ".join(
        f"{axis.name}={status.calibration[axis].positive.source}/{status.calibration[axis].negative.source}"
        for axis in LogicalAxis
    )


def _print_status(status: StageStatus) -> None:
    print(f"{status.side.name}: serial={status.serial_number}, resource={status.resource}")
    if not status.available:
        print("  unavailable")
        return
    observed = status.observed_voltage_v
    if observed is None:
        print("  logical voltages unavailable")
    else:
        print("  logical " + ", ".join(f"{axis.name}={observed[axis]:g} V" for axis in LogicalAxis))
    print(f"  baseline={'known' if status.baseline_known else 'unknown'}; nominal_authorized={status.nominal_authorized}")
    print(f"  calibration sources: {_sources(status)}")
    print(f"  restricted={status.restricted}; fault={status.fault}")


def _direction(status: StageStatus, axis: LogicalAxis, displacement_um: float) -> tuple[str, float]:
    toward = axis is LogicalAxis.X and displacement_um * status.toward_chip_sign > 0.0
    return ("toward chip", status.toward_chip_limit_um) if toward else ("away from chip", status.other_limit_um)


def _preview(setup: object, status: StageStatus, axis: LogicalAxis, displacement_um: float) -> tuple[object, float, bool]:
    """Return active public calibration, voltage delta, and exactness.

    Polarity is setup configuration data.  If a caller supplies only status
    data, report a conservative magnitude rather than guessing a sign.
    """
    polarity = None
    try:
        polarity = setup.config.stages[status.side].polarity[axis]
    except (AttributeError, KeyError, TypeError):
        pass
    coefficient = status.calibration[axis].positive if displacement_um > 0.0 else status.calibration[axis].negative
    has_public_polarity = type(polarity) is int and polarity in (-1, 1)
    if has_public_polarity:
        return coefficient, displacement_um * polarity / coefficient.um_per_v, True
    return coefficient, abs(displacement_um) / coefficient.um_per_v, False


def _stage_uses_nominal_calibration(status: StageStatus) -> bool:
    return any(
        coefficient.source == "nominal_MAX312D"
        for calibration in status.calibration.values()
        for coefficient in (calibration.positive, calibration.negative)
    )


def _print_plan(setup: object, status: StageStatus, axis: LogicalAxis, displacement_um: float, signed_um: str) -> None:
    classification, limit = _direction(status, axis, displacement_um)
    coefficient, delta_v, exact = _preview(setup, status, axis, displacement_um)
    precision = "estimated voltage delta" if exact else "conservative estimated voltage-delta magnitude"
    print(
        f"Planned {status.side.name} logical {axis.name} move: requested={signed_um} um; "
        f"{classification}; limit={limit:g} um; calibration={coefficient.source}; "
        f"{precision}={delta_v:g} V (coefficient={coefficient.um_per_v:g} um/V)."
    )
    print("No automatic return or rollback.")


def _confirm(expected: str) -> bool:
    print(f"Type exactly {expected}")
    try:
        actual = input("Confirmation: ")
    except EOFError:
        print("Confirmation declined; no motion setter was called.")
        return False
    if actual != expected:
        print("Confirmation declined; no motion setter was called.")
        return False
    return True


def _selected_stage(setup: object, side: StageSide) -> object:
    return setup.left if side is StageSide.LEFT else setup.right


def _run_read_only(setup: object) -> int:
    _print_status(setup.left.status)
    _print_status(setup.right.status)
    return 0


def _run_move(setup: object, side: StageSide, axis: LogicalAxis, displacement_um: float,
              signed_um: str, allow_nominal: bool) -> int:
    stage = _selected_stage(setup, side)
    status = stage.status
    _print_status(status)
    if not status.available:
        print(f"{side.name} is unavailable; baseline adoption was not attempted.")
        return 2
    _print_plan(setup, status, axis, displacement_um, signed_um)
    if _stage_uses_nominal_calibration(status) and not allow_nominal:
        print(
            "Nominal calibration is active on this stage; --allow-nominal is required "
            "before baseline adoption or motion."
        )
        return 2
    if not _confirm(f"ADOPT {side.name}"):
        return 2
    stage.adopt_baseline(confirm=True, allow_nominal=allow_nominal)
    current = stage.status
    _print_status(current)
    if not current.available:
        print(f"{side.name} became unavailable after adoption; no motion was attempted.")
        return 2
    _print_plan(setup, current, axis, displacement_um, signed_um)
    if not _confirm(f"MOVE {side.name} {axis.name} {signed_um} UM"):
        return 2
    vector = {
        LogicalAxis.X: Vector3Um(displacement_um, 0.0, 0.0),
        LogicalAxis.Y: Vector3Um(0.0, displacement_um, 0.0),
        LogicalAxis.Z: Vector3Um(0.0, 0.0, displacement_um),
    }[axis]
    result = stage.move_by_um(vector.x, vector.y, vector.z)
    if result.confirmed is not True:
        raise RuntimeError("public setup move result was not confirmed")
    observed = ", ".join(f"{logical.name}={result.observed_voltage_v[logical]:g} V" for logical in LogicalAxis)
    print(
        f"Move completed: requested={signed_um} um; estimated completed displacement="
        f"({result.estimated_after_um.x:g}, {result.estimated_after_um.y:g}, {result.estimated_after_um.z:g}) um; "
        f"final logical voltages: {observed}."
    )
    return 0


def _print_failure(error: Exception) -> None:
    print(f"Diagnostic failed: {type(error).__name__}: {error}")
    cleanup_error = getattr(error, "fiber_diagnostic_close_error", None)
    if cleanup_error is not None:
        print(f"Close also failed: {type(cleanup_error).__name__}: {cleanup_error}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    if args.move:
        if args.side is None or args.axis is None or args.um is None:
            parser.error("--move requires --side, --axis, and --um")
    elif args.side is not None or args.axis is not None or args.um is not None or args.allow_nominal:
        parser.error("--side, --axis, --um, and --allow-nominal require --move")

    try:
        if args.enumerate:
            discovery = FiberCouplingSetup.enumerate(**_setup_kwargs(args.config))
            _print_discovery(discovery)
            return 0
        if args.read_only:
            return _run_connected(args.config, _run_read_only)
        side, axis = StageSide(args.side), LogicalAxis(args.axis)
        signed_um = repr(args.um)
        return _run_connected(
            args.config,
            lambda setup: _run_move(setup, side, axis, args.um, signed_um, args.allow_nominal),
        )
    except Exception as error:
        _print_failure(error)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
