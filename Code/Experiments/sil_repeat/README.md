# Repeated SIL cycle scan

This experiment holds one laser-diode operating point while repeatedly scanning
the on-chip phase shifter forward and backward. It reuses the drivers in
`Code/Utils`; it never opens serial or VISA transports directly.

## Approved bench run

The checked-in configuration `repeat_24C_100mA.json` specifies:

- TEC target 24.0 °C and LD current 100.0 mA;
- Voltage Source channel 1;
- 40 uniformly spaced V² levels from 40 to 90 V², corresponding to
  6.3246–9.4868 V;
- 40 OSA observations in each direction;
- 100 complete cycles and 8,000 spectra total;
- 0.1 s settle after confirmed voltage telemetry; and
- OSA trace A with front-panel settings preserved.

The high turning voltage is measured once under each branch label so both
stored branches contain exactly 40 spectra. Because no voltage excursion
separates those two high-turning observations, the primary hysteresis metrics
exclude that pair and compare the remaining 39 voltage levels. All 80 raw
spectra remain available in every cycle directory.

At the observed OSA duration of about 1.46 s, the plan estimates about 3.5 hours
for the full run. The output may consume several hundred megabytes and is
written below `Result/sil_repeat` on the workspace drive, not the C: drive.

## Plan without touching hardware

Run from the repository root in the required environment:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -B -m Code.Experiments.sil_repeat.run --config .\Code\Experiments\sil_repeat\repeat_24C_100mA.json --plan --wait-for-coupling
```

`--plan` validates and prints the full schedule, estimated waits, resources,
capacity warning, output root, coupling gate, and shutdown order. It does not
create an output directory, a postprocessing process, or an instrument driver.

## Real run

Use this command only while an operator is physically at the bench:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -B -m Code.Experiments.sil_repeat.run --config .\Code\Experiments\sil_repeat\repeat_24C_100mA.json --wait-for-coupling
```

The CLI first requires exact uppercase `RUN`. Only then does it construct and
connect the OSA, Voltage Source, and Gain drivers. Every real run and resume
requires exact uppercase `COUPLED`; the legacy `--wait-for-coupling` flag is
accepted for command compatibility but cannot disable this gate. The Voltage Source remains at
0 V while the Gain TEC passes the five-second ±0.2 °C interlock. Current output
is then enabled at the controller's 3 mA reset value and ramped to 100 mA in
steps no larger than 1 mA separated by at least 0.1 s.

Real data roots are resolved against the repository and must remain within
`Result/sil_repeat`; a real config cannot redirect acquisition data elsewhere.
After the current is verified, the program again holds all Voltage Source
channels at 0 V and requires exact uppercase `COUPLED`. Any other response
shuts down without starting a scan. After `COUPLED`, the driver safely ramps to
the first 6.3246 V target and the 100 cycles run without disabling TEC/current
or reconnecting instruments between cycles.

Each completed cycle is queued to a separate postprocessing process. Raw
acquisition continues while the worker writes metrics plus SVG/PDF/PNG figures.
Only the latest-result window is refreshed; every earlier figure remains on
disk.

## Stop and resume

`Ctrl+C`, an OSA failure, a terminal communication fault, or any unhandled
exception stops later voltage commands. Cleanup attempts every action in this
order:

1. immediate all-channel Voltage Source zero;
2. Gain current disable;
3. Gain TEC disable;
4. Gain close;
5. Voltage Source close; and
6. OSA close.

Fully completed cycles remain reusable. A partial cycle is preserved for audit
but is never continued mid-branch. Resume starts a new attempt for that cycle
from an explicit 0 V state and requires fresh `RUN` and `COUPLED` authorization:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -B -m Code.Experiments.sil_repeat.run --resume .\Result\sil_repeat\run_<id> --wait-for-coupling
```

If every spectrum was acquired but figures are missing, resume regenerates the
missing products without constructing instruments.

## Output structure

```text
Result/sil_repeat/run_<id>/
├── run.json
├── experiment.log
├── cycles.csv
├── repeat_summary.csv
├── repeat_summary.svg/.pdf/.png
└── cycle_000/
    └── attempt_001/
        ├── status.json
        ├── acquisitions.csv
        ├── forward/000...039_*.npz
        ├── reverse/000...039_*.npz
        ├── metrics.csv
        ├── summary.json
        └── comparison.svg/.pdf/.png
```

The aggregate plot tracks P95 signal-region MAE and maximum absolute peak shift
against cycle number. The default classification is
`OBSERVATION_REQUIRED`; figures and metrics support operator judgment and do
not constitute an automatic locking-quality verdict.

## Offline verification

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -B -m unittest discover -s Code\Debugs -p 'test_sil_repeat_*.py' -v
```

The targeted suite uses fixed runner inputs and explicitly injected bounded
transport doubles. It exercises the real execution/storage contract without
opening actual hardware; no selectable instrument emulator is present.
Historical non-real manifests remain read-only and cannot resume acquisition.
