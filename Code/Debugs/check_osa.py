from __future__ import annotations

import argparse
import logging
import json
from dataclasses import asdict
from pathlib import Path

import numpy as np

from Code.Utils.osa import AQ6370


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check AQ6370 identity, read an existing trace, or explicitly start a sweep"
    )
    parser.add_argument("--resource", default="GPIB0::4::INSTR")
    parser.add_argument("--timeout", type=float, default=30.0)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--identity", action="store_true", help="Read *IDN? only (default)")
    modes.add_argument("--read-trace", metavar="A", help="Read an existing trace without changing the panel")
    modes.add_argument("--sweep", action="store_true", help="Explicitly start a new SINGLE/AUTO sweep")
    parser.add_argument("--out", type=Path, help="New directory for native trace evidence; never overwrite")
    args = parser.parse_args()
    if bool(args.read_trace) != bool(args.out):
        parser.error("--read-trace requires --out; --out is only for --read-trace")
    if args.out is not None:
        args.out.mkdir(parents=True, exist_ok=False)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    if not args.read_trace and not args.sweep:
        osa = AQ6370(resource_name=args.resource, timeout=args.timeout)
        try:
            report = osa.probe_identity()
            print(json.dumps({'identity': dict(report.identity),
                              'release_confirmed': report.release_confirmed}))
            return 0 if report.release_confirmed else 1
        finally:
            # probe_identity closes its identity-only resource without creating
            # a sweep instrument. Explicit retry still retains driver obligations.
            osa.close()
    if args.read_trace:
        osa = AQ6370(resource_name=args.resource, timeout=args.timeout)
        report = {"schema": 1, "kind": "osa_existing_trace", "resource": args.resource,
                  "trace": args.read_trace, "physical_state": "not_measured"}
        try:
            with osa:
                capture = osa.read_trace(args.read_trace, args.timeout)
            report.update({"status": "complete", "identity": capture.identity,
                           "trace": capture.trace, "sample_count": len(capture.native_values),
                           "native_unit": capture.native_unit, "consistency": capture.consistency,
                           "read_started_at": capture.read_started_at.isoformat(),
                           "read_finished_at": capture.read_finished_at.isoformat(),
                           "elapsed_s": capture.elapsed_s,
                           "context_before": asdict(capture.context_before),
                           "context_after": asdict(capture.context_after),
                           "release_confirmed": not osa.has_resource_responsibility})
            with (args.out / "trace.npz").open("xb") as stream:
                np.savez(stream, wavelength_nm=capture.wavelength_nm,
                         native_values=capture.native_values)
        except BaseException as error:
            report.update({"status": "failed", "error_type": type(error).__name__,
                           "release_confirmed": not osa.has_resource_responsibility})
            try:
                with (args.out / "trace.json").open("x", encoding="utf-8") as stream:
                    json.dump(report, stream, indent=2, allow_nan=False)
            except BaseException as save_error:
                osa._attach_evidence(error, "evidence_save_error", save_error)
            raise
        with (args.out / "trace.json").open("x", encoding="utf-8") as stream:
            json.dump(report, stream, indent=2, allow_nan=False)
        print(json.dumps(report, allow_nan=False))
        return 0
    with AQ6370(resource_name=args.resource, timeout=args.timeout) as osa:
        spectrum = osa.acquire()
        print(f"identity={osa.identity}")
        print(f"points={len(spectrum.wavelength_nm)}")
        print(
            f"wavelength_nm={spectrum.wavelength_nm[0]:.6f}.."
            f"{spectrum.wavelength_nm[-1]:.6f}"
        )
        print(f"power_dbm={np.min(spectrum.power_dbm):.3f}..{np.max(spectrum.power_dbm):.3f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
