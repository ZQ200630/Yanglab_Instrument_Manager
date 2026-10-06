# SIL forward/reverse hysteresis experiment

This package runs one continuous forward/reverse phase-shifter scan at each
laser-diode operation point, saves every OSA spectrum immediately, and compares
the paired branches without resampling. Execution is real-only; start with
configuration planning and hardware-free safety regressions.

> **Hardware warning:** Do not start a real run until the instrument identities,
> ports/resources, optical path, electrical limits, interlocks, and emergency
> shutdown behavior have been checked by the operator. A real run changes the
> Gain TEC and laser-current outputs and scans one Voltage Source channel. The
> commands below do not authorize hardware use.

## Environment and commands

Run from the repository root with the required offline environment:

```powershell
$visaPython = 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe'
```

Validate and print the default plan without creating any instrument or output
directory:

```powershell
& $visaPython -B -m Code.Experiments.sil_hysteresis.run --plan
```

Run the hardware-free regression tests before any staged hardware diagnostic:

```powershell
& $visaPython -B -m unittest discover -s Code/Debugs -p 'test_*.py' -q
```

Use another validated JSON file with `--config PATH`. The `--config` and
`--resume` options are mutually exclusive.

## Approved defaults

[`default_config.json`](default_config.json) defines the approved first run:

- one operation point: exactly 22.0 °C and 80.0 mA;
- Voltage Source channel 1, from 0 to 13 V;
- 100 points uniformly spaced in V²;
- 0.1 s settle time after each voltage command;
- OSA trace A, with front-panel measurement settings preserved;
- a 30 dB union-of-branches signal mask;
- no classification thresholds; and
- the latest-result window enabled unless `--no-display` is supplied.

The schedule is a single continuous triangle: 100 forward acquisitions include
13 V, then 99 reverse acquisitions begin at the second-highest voltage and end
at 0 V. Thus each operation point has `2 × points - 1 = 199` acquisitions. The
sole 13 V turning observation remains in raw storage but is excluded from paired
branch statistics because there is no independent reverse acquisition there.

For a shorter plan preview or explicitly operator-approved diagnostic, copy the
default JSON and reduce `scan.points`; do not change the installed default. A
value of `N` produces `2N - 1` acquisitions, and `N` must be at least 2. Reducing
the point count changes sampling density and is not equivalent to the approved
100-point experiment.

## Multiple operation points

Use exactly one of an explicit list or a grid. Explicit points retain the listed
order:

```json
"gain": {
  "operation_points": [
    {"temperature_c": 21.0, "current_ma": 70.0},
    {"temperature_c": 22.0, "current_ma": 80.0}
  ]
}
```

A grid expands temperature-major, then current:

```json
"gain": {
  "grid": {
    "temperature_c": [21.0, 22.0],
    "current_ma": [70.0, 80.0]
  }
}
```

That grid runs `(21,70)`, `(21,80)`, `(22,70)`, then `(22,80)`. Temperatures
must remain within 15–40 °C and currents within 0–200 mA. The driver still owns
all stability and current-enable interlocks.

[`grid_4x4.json`](grid_4x4.json) is the bench configuration for the 16-point
Cartesian product of temperatures `[22, 24, 26, 28]` °C and currents
`[100, 115, 130, 145]` mA. Its fast-screening V² scan uses 21 forward levels,
one unpaired 13 V turning point, and 20 reverse levels, yielding 20 paired
forward/reverse comparisons while retaining all default safety settings.

## Real-hardware gate

First inspect the exact plan:

```powershell
& $visaPython -B -m Code.Experiments.sil_hysteresis.run --config .\my_config.json --plan
```

Only after separate operator authorization, start the real CLI:

```powershell
& $visaPython -B -m Code.Experiments.sil_hysteresis.run --config .\my_config.json --wait-for-coupling
```

The CLI validates and prints every operation point, resource, voltage limit,
acquisition count, wait, timing lower bound, output location, and shutdown order
before driver construction. It then prompts:

```text
Type exactly RUN to construct real instruments:
```

Only the exact, case-sensitive token `RUN` proceeds. Any other input cancels
without constructing instruments. Obsolete backend flags are rejected before
config loading or driver construction; there is no alternate acquisition backend.

After the TEC has passed the five-second stability interlock, the driver enables
the laser-current output. This Gain controller resets its current setpoint to
3 mA at that instant, so the software reads back the actual 3 mA state and then
ramps to the configured target in steps no larger than
`current_ramp_step_ma`, separated by at least `current_ramp_interval_s`. The
default is 1 mA every 0.1 s. A failed readback, temperature excursion, or lost
TEC/current-enable state trips the normal safe shutdown.

With `--wait-for-coupling`, the scan voltage is re-zeroed after the target
current is verified. TEC and laser current remain active while the operator
couples the chip. The scan starts only after the exact, case-sensitive token
`COUPLED`; any other input triggers safe shutdown without an OSA acquisition.

For an unattended multi-point run after one initial optical alignment, use
`--wait-for-first-coupling` instead. It pauses at the first pending operation
only; after exact `COUPLED`, all later operation points run automatically. The
original `--wait-for-coupling` behavior remains available when confirmation is
required at every point, and the two options are mutually exclusive.

```powershell
& $visaPython -B -m Code.Experiments.sil_hysteresis.run --config .\Code\Experiments\sil_hysteresis\grid_4x4.json --wait-for-first-coupling
```

Before repeating a real scan after a Voltage Source communication fault, a
zero-output telemetry soak can be run without constructing the Gain or OSA
drivers:

```powershell
& $visaPython -B -m Code.Debugs.check_voltage --monitor-zero-seconds 300
```

This diagnostic writes only the driver's startup/shutdown all-channel 0 V
frames, verifies every observed channel remains within the 0 V tolerance, and
monitors fresh telemetry for the requested duration. It does not enter the
interactive 0.1 V check.

## Timing and rolling ETA

For the default single point, the printed known lower-bound components are:

- 199 configured acquisition settles × 0.1 s = 19.9 s;
- 7.2 s minimum cumulative voltage-ramp wait for the driver’s ≤0.1 V steps and
  ≥50 ms inter-step interval;
- 7.7 s to ramp the enabled output from 3 mA to 80 mA at 1 mA per 0.1 s; and
- 5.0 s configured laser-diode settle after the current ramp.

Temperature stabilization can take up to the configured 300 s timeout. OSA
sweep time is unknown before acquisition 1. After each spectrum, the logger
updates the measured mean OSA duration and estimates remaining time as:

```text
remaining OSA sweeps × measured mean OSA time
+ remaining configured settle time
+ conservative remaining voltage-ramp time
```

This is a rolling estimate, not a deadline. Real front-panel OSA settings and
temperature dynamics determine the actual duration.

## Output and traceability

Each invocation creates a unique run below `Result/sil_hysteresis` (or the
configured `output_root`):

```text
run_<UTC timestamp>_<id>/
├── run.json                    # frozen config, 199-step schedule, provenance
├── experiment.log             # timestamped acquisition and cleanup audit
└── op_000_T22.000_I100.000/
    └── attempt_001/
        ├── status.json         # running / failed / complete attempt manifest
        ├── acquisitions.csv    # scalar telemetry and NPZ path per acquisition
        ├── forward/            # 100 atomic spectrum NPZ files by default
        ├── reverse/            # 99 atomic spectrum NPZ files by default
        ├── metrics.csv         # 99 paired rows; no shared 13 V turning row
        ├── summary.json        # definitions, exclusions, ranking, classification
        ├── comparison.svg      # primary editable figure
        ├── comparison.pdf      # editable publication figure
        └── comparison.png      # 300 dpi preview
```

Each spectrum NPZ contains:

- raw arrays: `wavelength_nm`, `power_dbm`;
- schedule identity: `requested_voltage_v`, `direction`, `direction_index`,
  `sequence_index`, and `comparison_index` (`-1` for the 13 V turning point);
- all-channel snapshots: `measured_voltage_v`, `measured_current_ma`, plus
  `measured_voltage_1_v` through `measured_voltage_8_v` and
  `measured_current_1_ma` through `measured_current_8_ma`;
- OSA audit fields: `acquired_at`, `osa_duration_s`, `trace`, and `identity`;
- Gain audit fields: `measured_temperature_c`, `target_temperature_c`,
  `tec_enabled`, `current_setpoint_ma`, and `current_output_enabled`;
- telemetry times: `voltage_status_received_at`, `gain_status_received_at`; and
- provenance: `provenance_label=REAL`, `run_mode=real`, and `simulated=false`.
  Historical records retain their original source labels; they are not rewritten
  or eligible for new acquisition.

`metrics.csv` is derived directly from the paired raw NPZ files. It records the
full- and signal-region MAE/RMS/correlation, signal-bin count, branch peak
wavelengths, and peak shift at each independently paired voltage. `summary.json`
records the exact mask rule, aggregate definitions, the one turning-point
exclusion, the five largest signal-region discrepancies, and run provenance.
The figure reads the same in-memory comparison result used to write these files.

## Resume and incomplete attempts

Resume only an existing real run. For example:

```powershell
& $visaPython -B -m Code.Experiments.sil_hysteresis.run --resume .\Result\sil_hysteresis\run_<real-id> --no-display
```

A historical non-real source is rejected before authorization or instrument
construction. Existing source labels remain readable through the data loaders.
A complete operation point is revalidated from its manifest and every referenced
NPZ, then skipped. If raw acquisition is complete but a postprocessing artifact
is missing, resume regenerates only metrics/figures without constructing devices.

An incomplete triangular scan is never continued mid-branch. Partial spectra are
preserved in the failed attempt for diagnosis, but the next attempt gets a new
number and restarts from an explicit all-channel 0 V state. This preserves one
continuous voltage history and prevents scientifically invalid pairing across
separate thermal, laser, or phase-shifter histories.

## Interpretation and display behavior

With `analysis.thresholds` set to `null`, every result is
`OBSERVATION_REQUIRED`. This means numerical indicators and the largest paired
discrepancies are available for review; it is **not** a locking-quality verdict,
acceptance decision, or calibrated scientific pass/fail claim. Optional named
thresholds produce exploratory labels only and require independent scientific
justification.

Without `--no-display`, one non-blocking Matplotlib window is created or reused
for only the latest completed operation point. Earlier figures remain on disk.
The window is updated only after the scan is marked complete, voltage is zeroed,
and Gain current is disabled. A missing or failed GUI logs a warning and disables
later display attempts without failing the experiment. Use `--no-display` on a
headless machine.

## Failure and shutdown behavior

OSA acquisition is fail-fast: no later voltage is acquired after a failed sweep.
Every successful spectrum already saved remains on disk, the attempt is marked
failed when possible, and the operation is scientifically incomplete. Cleanup
attempts every step in this order even if one cleanup action fails:

1. request immediate eight-channel Voltage Source zero;
2. disable Gain current;
3. disable Gain TEC;
4. close Gain;
5. close the Voltage Source; and
6. close the OSA.

The primary acquisition error is preserved if cleanup also fails. Experiment
code never opens serial or VISA resources directly and never changes OSA span,
RBW, sample count, sensitivity, or sweep mode.

Voltage Source communication uses a bounded recovery policy. A transient serial
read or malformed telemetry frame pauses the next voltage command and OSA
acquisition while leaving the last successfully commanded output unchanged. A
valid telemetry frame within 3 s resumes the same scan point. Each failed or
short command write is retried as the exact same eight-channel frame, up to three
total attempts. OSA acquisition begins only after a post-command telemetry frame
confirms the selected channel is within 0.10 V of its target. This operator-selected
tolerance covers the approximately 50 mV stable readback offset observed on the
bench and equals one maximum normal 0.1 V ramp step. Recovery attempts,
elapsed time, and success are written to `experiment.log`.

If telemetry does not recover within 3 s, or all three writes fail, the driver
enters `FAULT` and immediately requests all-channel 0 V. Invalid/over-limit
commands, operator interruption, and non-communication safety faults are never
retried. The standard shutdown order above still runs after a terminal fault.
