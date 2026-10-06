"""Run the three instrument safe-state health checks in a controlled order."""

from __future__ import annotations

import argparse
import logging
from collections.abc import Callable

from Code.Utils import AQ6370, GainDriver, VoltageSource


def run_checks(
    osa_factory: Callable[[], object] = AQ6370,
    voltage_factory: Callable[[], object] = VoltageSource,
    gain_factory: Callable[[], object] = GainDriver,
) -> dict[str, object]:
    """Check OSA, voltage source, then gain driver, closing each before the next.

    This performs a safe-state health check: OSA acquisition triggers a sweep,
    VoltageSource connection performs startup zero and cleanup returns all
    channels to zero, and GainDriver cleanup disables current before TEC.
    Callers must obtain approval for those state changes before invoking this
    function. Factories are injectable so ordering and cleanup can be tested
    offline without opening hardware.
    """
    results: dict[str, object] = {}

    with osa_factory() as osa:
        spectrum = osa.acquire()
        wavelengths = spectrum.wavelength_nm
        if len(wavelengths) == 0:
            raise ValueError("OSA acquisition returned an empty spectrum")
        results["osa"] = {
            "identity": osa.identity,
            "points": len(wavelengths),
            "wavelength_nm": (float(wavelengths[0]), float(wavelengths[-1])),
        }

    with voltage_factory() as voltage:
        status = voltage.read_status()
        results["voltage"] = {
            "voltage_v": status.voltage_v,
            "current_ma": status.current_ma,
        }

    with gain_factory() as gain:
        status = gain.status
        results["gain"] = {
            "temperature_c": status.temperature_c,
            "target_c": status.target_c,
            "tec_enabled": status.tec_enabled,
            "current_ma": status.current_ma,
            "current_enabled": status.current_enabled,
        }

    return results


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run safe-state health checks for OSA, voltage, and gain drivers."
    )
    parser.add_argument("--resource", default="GPIB0::4::INSTR", help="AQ6370 VISA resource")
    parser.add_argument("--osa-timeout", type=float, default=30.0)
    parser.add_argument("--voltage-port", help="CH340 serial port")
    parser.add_argument("--gain-port", help="CP210x serial port")
    parser.add_argument("--gain-serial-number", help="preferred CP210x USB serial number")
    parser.add_argument(
        "--confirm-state-changes",
        action="store_true",
        help="explicitly authorize the OSA sweep and safe shutdown state changes",
    )
    args = parser.parse_args()

    if not args.confirm_state_changes and not _confirm_state_changes():
        print("Cancelled; no instrument factories were constructed.")
        return 0

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
    )

    def osa_factory() -> AQ6370:
        return AQ6370(resource_name=args.resource, timeout=args.osa_timeout)

    def voltage_factory() -> VoltageSource:
        return VoltageSource(port=args.voltage_port)

    def gain_factory() -> GainDriver:
        kwargs = {"port": args.gain_port}
        if args.gain_serial_number is not None:
            kwargs["usb_serial"] = args.gain_serial_number
        return GainDriver(**kwargs)

    results = run_checks(osa_factory, voltage_factory, gain_factory)
    print("OSA:", results["osa"])
    print("Voltage:", results["voltage"])
    print("Gain:", results["gain"])
    return 0


def _confirm_state_changes() -> bool:
    """Ask for explicit interactive approval before constructing any device."""
    prompt = (
        "This safe-state health check will trigger one OSA sweep, perform "
        "Voltage Source startup zero and final all-channel zero, and ensure "
        "Gain Driver current-off then TEC-off cleanup. Continue? [y/N] "
    )
    try:
        return input(prompt).strip().lower() == "y"
    except (EOFError, KeyboardInterrupt):
        return False


if __name__ == "__main__":
    raise SystemExit(main())
