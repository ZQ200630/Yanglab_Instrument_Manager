# SIL Repeated-Cycle Scan Design

## Goal

Add a dedicated, scalable experiment for repeatedly measuring forward/reverse
self-injection-locking hysteresis at one fixed laser-diode operating point. The
first approved run holds the laser diode at 24.0 °C and 100.0 mA, scans 40
uniformly spaced V² levels from 40 to 90 V² in each direction, and records 100
complete forward/reverse cycles (8,000 OSA spectra total).

This is a new experiment under `Code/Experiments/sil_repeat`. It reuses the
existing drivers and analysis/figure conventions without changing the proven
single-triangle `sil_hysteresis` experiment or its output schema.

## Approved acquisition semantics

- Convert the inclusive V² grid `linspace(40, 90, 40)` to commanded voltage
  with `V = sqrt(V²)`. The electrical range is therefore approximately
  6.3246–9.4868 V.
- A cycle contains exactly 40 forward acquisitions from low to high followed
  by exactly 40 reverse acquisitions from high to low.
- The high turning voltage is acquired once with the forward label and again
  with the reverse label. The low voltage is likewise acquired at the end of
  reverse and again at the beginning of the next forward pass. These duplicate
  commands intentionally provide exactly 40 observations per labeled branch;
  each remains a separate OSA acquisition even when the voltage command does
  not change.
- All 80 spectra are stored for each cycle. The high turning pair is excluded
  from the primary branch-hysteresis aggregate because no voltage excursion
  separates its two observations. It remains visible in raw data and per-level
  tables. The remaining 39 voltage pairs are independently comparable.
- The TEC, laser-current output, and instrument sessions stay active between
  cycles. No recoupling or current re-enable occurs between cycles.

## Architecture

`sil_repeat` has five focused components:

1. `config.py` validates the fixed operating point, V² limits, 40 points,
   cycle count, resources, waits, and existing driver safety bounds.
2. `scan.py` builds an immutable 8,000-step schedule. Every step carries a
   global sequence index, cycle index, direction, branch index, V² value,
   commanded voltage, and pair index.
3. `storage.py` owns an append-oriented run manifest and one bounded directory
   per cycle. It never rewrites a manifest containing all 8,000 acquisition
   records.
4. `analysis.py` compares the two branches of one completed cycle and appends
   one row of cycle-level metrics to the run summary. It reuses the numerical
   definitions and signal mask from `sil_hysteresis` where applicable.
5. `run.py` and `experiment.py` own the authorization gates, driver lifecycle,
   acquisition loop, progress/ETA, background postprocessing queue, and safe
   shutdown.

Experiment code constructs instruments only through drivers in `Code/Utils`.
It never opens serial or VISA resources directly.

## Output and incremental durability

Each invocation creates a unique directory below `Result/sil_repeat`:

```text
run_<UTC timestamp>_<id>/
├── run.json
├── experiment.log
├── cycles.csv
├── repeat_summary.csv
├── repeat_summary.png/.pdf/.svg
└── cycle_000/
    └── attempt_001/
        ├── status.json
        ├── acquisitions.csv
        ├── forward/000...039_*.npz
        ├── reverse/000...039_*.npz
        ├── metrics.csv
        ├── summary.json
        └── comparison.png/.pdf/.svg
```

The run-level `cycles.csv` contains one small row per cycle and is appended
atomically after a cycle becomes terminal. Each cycle's `status.json` contains
at most 80 acquisition records, avoiding the quadratic 8,000-record rewrite
behavior of the single-attempt storage model. Every successful spectrum is
written atomically before its scalar metadata is appended.

The latest completed cycle is queued to one postprocessing worker. Acquisition
continues without waiting for SVG/PDF/PNG export, so figure generation does not
add an artificial dwell at the low-voltage boundary. The worker saves every
cycle's metrics and figure and refreshes only the latest-result window. At
normal completion the main process drains the queue, validates all products,
and writes the aggregate 100-cycle trend figure.

## Aggregate analysis

For every cycle, persist at least:

- P95 signal-region MAE;
- maximum absolute peak shift;
- the V² coordinate of the largest signal discrepancy;
- mean OSA acquisition duration;
- start/completion time; and
- terminal state and provenance.

The aggregate figure plots P95 signal-region MAE and maximum absolute peak
shift against cycle number. It is an observation aid, not an automatic
good/bad verdict; default classification remains `OBSERVATION_REQUIRED`.

## Hardware lifecycle and safety

The real CLI first prints the full plan and requires exact `RUN`. Driver
construction then begins with identity/read-only connection behavior. The
Voltage Source is held at 0 V while the TEC reaches 24.0 °C and remains within
the driver's ±0.2 °C interlock for five seconds. Current output is enabled at
the controller's 3 mA reset value and ramped to 100 mA in steps no larger than
1 mA with at least 0.1 s between steps. The CLI then requires exact `COUPLED`.

After coupling, the Voltage Source ramps from 0 V to the first 6.3246 V target
through the existing driver. All normal voltage motion remains limited to
steps no larger than 0.1 V and at least 50 ms between steps. OSA acquisition
starts only after fresh post-command telemetry confirms the selected channel
within 100 mV of target. Existing bounded write retry and transient telemetry
recovery policies remain authoritative.

Normal completion, operator interruption, acquisition failure, and terminal
communication faults all execute the established cleanup order: immediate
all-channel voltage zero, Gain current disable, Gain TEC disable, Gain close,
Voltage Source close, then OSA close. Cleanup attempts every step without
masking the primary error.

## Failure and resume semantics

- A failed OSA acquisition stops the hardware loop; no later voltage is
  acquired.
- Fully completed cycles and spectra from the incomplete cycle remain on disk.
- A partial cycle is never continued mid-branch. Resume creates a new attempt
  for that cycle and restarts it from an explicit 0 V state after a fresh
  `RUN`, TEC/current interlock, and `COUPLED` authorization.
- Resume revalidates every referenced spectrum in completed cycles before
  skipping them.
- If acquisition finished but cycle figures are missing, resume performs only
  the missing postprocessing for that cycle.

## Timing and capacity

At the observed OSA duration of about 1.46 s, 8,000 OSA sweeps require about
3.24 hours. Configured 0.1 s settles add about 13.3 minutes, and voltage/current
ramps plus setup and postprocessing bring the expected wall time to roughly
3.5 hours. Rolling ETA uses the measured OSA mean and remaining conservative
driver ramp waits.

The run plan reports the exact 8,000 acquisitions and warns that the result may
consume several hundred megabytes. No generated output is written to
`Code/Utils` or to the C: drive; experiment output stays under
`Result/sil_repeat` on the workspace drive.

## Verification

All Python checks run in the Anaconda `VISA` environment. Targeted offline
tests cover:

- exact 100 × 2 × 40 schedule length and direction/cycle indices;
- V² endpoints and uniform spacing;
- endpoint duplication and high-turning exclusion from primary metrics;
- bounded per-cycle storage and atomic completion;
- completed-cycle resume and partial-cycle restart;
- background postprocessing drain and aggregate rows;
- deterministic simulated end-to-end acquisition without constructing real
  drivers; and
- cleanup order under normal completion, interruption, and injected failure.

Before real hardware use, the CLI `--plan` output must confirm 24.0 °C,
100.0 mA, V² 40–90, 40 points per direction, 100 cycles, 8,000 acquisitions,
the coupling gate, estimated duration, output directory, and shutdown order.

## Acceptance criteria

The feature is ready for the approved real run when the targeted VISA tests
pass, the simulation produces 100 complete cycle directories and the aggregate
summary, the real plan is exact, and no real driver is constructed during
offline verification. The hardware run starts only after a separate operator
`RUN` authorization and begins scanning only after `COUPLED`.
