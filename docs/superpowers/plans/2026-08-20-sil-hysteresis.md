# SIL Hysteresis Automation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a safe, configuration-driven experiment that holds each laser-diode temperature/current operation point, performs one continuous voltage-squared-spaced forward/reverse phase-shifter scan, saves every OSA spectrum immediately, and produces a Nature-style comparison figure and exploratory consistency analysis.

**Architecture:** Add a focused `Code/Experiments/sil_hysteresis` package that depends only on the public `Code/Utils` driver interfaces. Separate immutable configuration and scan planning, crash-safe storage, paired-spectrum analysis, publication-style plotting, orchestration, simulation, and CLI concerns so each boundary can be tested offline. The first release defaults to one 22 °C / 80 mA operation point but uses the same pipeline for explicit lists or temperature-current grids.

**Tech Stack:** Python in the Anaconda `VISA` environment, standard library (`argparse`, `csv`, `dataclasses`, `json`, `logging`, `pathlib`, `time`, `unittest`), NumPy, Matplotlib, existing `Code.Utils` drivers, installed `nature-figure` workflow.

**Spec:** Self-contained plan derived from the design approved in chat on 2026-08-20; the user explicitly requested proceeding directly to the implementation plan without a separate spec document.

## Global Constraints

- Do not modify or bypass safety behavior in `Code/Utils/gain.py`, `Code/Utils/voltage.py`, or `Code/Utils/osa.py`.
- Experiment code must never open serial ports or VISA resources directly; it uses `GainDriver`, `VoltageSource`, and `AQ6370` only.
- Default operation point is exactly 22.0 °C and 80.0 mA.
- Gain settings remain within 15–40 °C and 0–200 mA; current is enabled only through `GainDriver.enable_current()` after `wait_stable()` succeeds.
- Default phase-shifter scan is channel 1, 0–13 V, 100 points uniformly spaced in V², and `settle_time_s=0.1`.
- Normal voltage motion stays under the driver's ≤0.1 V step and ≥50 ms interval rules; all handled failures request immediate eight-channel zero.
- A scan is one continuous triangular history: forward includes all 100 points; reverse begins at the second-highest point, yielding 199 acquisitions total. The shared 13 V turning point is excluded from independent branch-difference statistics.
- Preserve all OSA front-panel measurement settings. The experiment may select the configured trace and call `acquire()` but must not set span, RBW, sample count, sensitivity, or sweep mode.
- OSA acquisition failure is fail-fast. Preserve completed files, zero voltage, disable Gain current, disable TEC during final shutdown, and never continue a scientifically incomplete scan.
- Never resume in the middle of an incomplete triangular scan. Skip complete operation points; restart an incomplete operation point in a new numbered attempt from 0 V.
- Save every successful spectrum atomically before proceeding to the next voltage.
- Plot exclusively with Python/matplotlib. Save editable SVG as the primary figure plus PDF and 300 dpi PNG. Preserve all source observations and report all exclusions or masks.
- The first release does not make a calibrated scientific pass/fail claim. With no user-supplied thresholds it reports `OBSERVATION_REQUIRED`, numerical indicators, and the largest discrepancies. Optional later thresholds may produce exploratory labels.
- A single non-blocking Matplotlib window shows only the latest completed operation point. All prior figures remain on disk. A missing GUI produces a warning, not an experiment failure.
- Before any real-device construction, validate the full configuration, print operation points, voltage limits, acquisition count, and timing lower bound, then require explicit operator confirmation.
- Run all offline tests with `D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe` before any hardware validation.
- The workspace currently has no Git repository. At each commit checkpoint, run `git rev-parse --is-inside-work-tree`; commit only if it succeeds, otherwise log `SKIPPED: workspace is not a Git repository` and continue without initializing Git.

## Planned File Structure

```text
Code/Experiments/
├── __init__.py
└── sil_hysteresis/
    ├── __init__.py          # Stable package exports only
    ├── config.py            # Immutable config models, JSON parsing, validation
    ├── scan.py              # V² grid, triangular schedule, timing estimates
    ├── storage.py           # Run/attempt directories and atomic checkpoints
    ├── analysis.py          # Branch alignment, masks, paired metrics, summaries
    ├── figure.py            # Nature-style figure and reusable latest-result window
    ├── experiment.py        # Safe instrument orchestration and fail-fast lifecycle
    ├── simulate.py          # Interface-compatible synthetic instruments
    ├── run.py               # CLI, plan display, confirmation, logging
    ├── default_config.json  # Approved single-point defaults
    └── README.md            # Operator workflow and output interpretation
Code/Debugs/
├── test_sil_config_scan.py
├── test_sil_storage.py
├── test_sil_analysis_figure.py
└── test_sil_experiment_cli.py
Result/sil_hysteresis/       # Generated only at runtime; never committed
```

---

### Task 1: Immutable Configuration and Safe Scan Planning

**Files:**
- Create: `Code/Experiments/__init__.py`
- Create: `Code/Experiments/sil_hysteresis/__init__.py`
- Create: `Code/Experiments/sil_hysteresis/config.py`
- Create: `Code/Experiments/sil_hysteresis/scan.py`
- Create: `Code/Experiments/sil_hysteresis/default_config.json`
- Test: `Code/Debugs/test_sil_config_scan.py`

**Interfaces:**
- Produces: `OperationPoint`, `DeviceConfig`, `ScanConfig`, `AnalysisConfig`, `RunConfig`, `load_config(path: Path) -> RunConfig`.
- Produces: `ScanStep`, `build_voltage_grid(scan: ScanConfig) -> np.ndarray`, `build_scan_steps(scan: ScanConfig) -> tuple[ScanStep, ...]`, and `TimingTracker`.
- Later tasks consume the immutable config and schedule; no later task parses raw JSON dictionaries.

- [ ] **Step 1: Write failing tests for approved defaults and JSON round-trip**

```python
class ConfigTests(unittest.TestCase):
    def test_default_config_matches_approved_single_point(self):
        config = load_config(DEFAULT_CONFIG)
        self.assertEqual(config.operation_points, (OperationPoint(22.0, 80.0),))
        self.assertEqual(config.scan.channel, 1)
        self.assertEqual(config.scan.v_min, 0.0)
        self.assertEqual(config.scan.v_max, 13.0)
        self.assertEqual(config.scan.points, 100)
        self.assertEqual(config.scan.spacing, "v_squared")
        self.assertEqual(config.scan.settle_time_s, 0.1)

    def test_grid_and_explicit_points_are_mutually_exclusive(self):
        payload = valid_payload()
        payload["gain"]["grid"] = {"temperature_c": [21, 22], "current_ma": [70, 80]}
        with self.assertRaisesRegex(ValueError, "exactly one"):
            RunConfig.from_mapping(payload)
```

- [ ] **Step 2: Run the new test module and verify imports fail**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_config_scan -v
```

Expected: FAIL because `Code.Experiments.sil_hysteresis.config` and `scan` do not exist.

- [ ] **Step 3: Implement frozen configuration dataclasses and exact validation**

Implement these public shapes:

```python
@dataclass(frozen=True)
class OperationPoint:
    temperature_c: float
    current_ma: float

@dataclass(frozen=True)
class DeviceConfig:
    osa_resource: str = "GPIB0::4::INSTR"
    osa_trace: str = "A"
    voltage_port: str | None = None
    gain_port: str | None = None
    gain_serial_number: str | None = None

@dataclass(frozen=True)
class ScanConfig:
    channel: int = 1
    v_min: float = 0.0
    v_max: float = 13.0
    points: int = 100
    spacing: str = "v_squared"
    settle_time_s: float = 0.1

@dataclass(frozen=True)
class AnalysisConfig:
    signal_window_db: float = 30.0
    display_floor_dbm: float | None = None
    thresholds: Mapping[str, float] | None = None

@dataclass(frozen=True)
class RunConfig:
    devices: DeviceConfig
    scan: ScanConfig
    operation_points: tuple[OperationPoint, ...]
    temperature_timeout_s: float = 300.0
    ld_settle_s: float = 5.0
    analysis: AnalysisConfig = AnalysisConfig()
    output_root: Path = Path("Result/sil_hysteresis")
    display_latest: bool = True
```

Validation must reject booleans as numbers, NaN/infinity, temperatures outside 15–40, currents outside 0–200, channels outside 1–8, voltages outside 0–14, `v_min >= v_max`, fewer than two points, spacing other than `v_squared`, negative waits, invalid OSA traces, empty operation-point collections, duplicate explicit pairs, and simultaneous/missing explicit-list and grid definitions. Grid expansion order is temperature-major, then current.

- [ ] **Step 4: Add failing scan-schedule and timing tests**

```python
class ScanTests(unittest.TestCase):
    def test_voltage_grid_is_uniform_in_squared_voltage(self):
        grid = build_voltage_grid(ScanConfig(points=100))
        np.testing.assert_allclose(grid**2, np.linspace(0.0, 169.0, 100))
        self.assertEqual(grid[0], 0.0)
        self.assertEqual(grid[-1], 13.0)

    def test_schedule_is_continuous_and_has_199_acquisitions(self):
        steps = build_scan_steps(ScanConfig(points=100))
        self.assertEqual(len(steps), 199)
        self.assertEqual([s.direction for s in steps[:100]], ["forward"] * 100)
        self.assertEqual(steps[99].voltage_v, 13.0)
        self.assertLess(steps[100].voltage_v, 13.0)
        self.assertEqual(steps[-1].voltage_v, 0.0)
```

- [ ] **Step 5: Implement the grid, schedule, and rolling ETA**

`ScanStep` contains `sequence_index`, `direction`, `direction_index`, `voltage_v`, and `comparison_index`. Forward comparison indices are 0 through `N-2`; the turning point has `comparison_index=None`; reverse indices map exactly to the matching ascending-voltage row. `TimingTracker.record_acquisition(seconds)` maintains count, total, and mean; `estimate_remaining(remaining_acquisitions, remaining_settle_s, remaining_voltage_ramp_s)` returns `None` until one OSA timing exists.

- [ ] **Step 6: Run configuration and scan tests**

Run the Task 1 unittest command. Expected: all tests PASS.

- [ ] **Step 7: Commit if Git is available**

```powershell
git rev-parse --is-inside-work-tree
git add Code/Experiments Code/Debugs/test_sil_config_scan.py
git commit -m "feat: add SIL hysteresis configuration and scan plan"
```

If the first command fails, do not initialize Git; record the required `SKIPPED` message.

---

### Task 2: Crash-Safe Per-Spectrum Storage and Attempt Semantics

**Files:**
- Create: `Code/Experiments/sil_hysteresis/storage.py`
- Test: `Code/Debugs/test_sil_storage.py`

**Interfaces:**
- Consumes: `RunConfig`, `OperationPoint`, and `ScanStep` from Task 1; `Spectrum`, `VoltageStatus`, and `GainStatus` from `Code.Utils`.
- Produces: `RunStore.create(config)`, `RunStore.open_existing(path)`, `AttemptStore.save_acquisition(...)`, `AttemptStore.mark_complete()`, and `AttemptStore.mark_failed(error)`.
- Produces: `StoredAcquisition` metadata and `load_complete_attempt(path) -> CompletedAttemptData` for analysis.

- [ ] **Step 1: Write failing tests for atomic spectrum files and metadata**

```python
def test_save_acquisition_writes_arrays_and_confirmed_metadata(self):
    with tempfile.TemporaryDirectory() as root:
        attempt = make_attempt(Path(root))
        saved = attempt.save_acquisition(
            step=ScanStep(0, "forward", 0, 0.0, 0),
            spectrum=fake_spectrum(),
            voltage_status=fake_voltage_status(),
            gain_status=fake_gain_status(),
            osa_duration_s=1.25,
        )
        self.assertTrue(saved.path.exists())
        self.assertFalse(any(saved.path.parent.glob("*.tmp")))
        with np.load(saved.path) as payload:
            np.testing.assert_array_equal(payload["wavelength_nm"], [1550.0, 1550.1])
            np.testing.assert_array_equal(payload["power_dbm"], [-40.0, -20.0])
            self.assertEqual(float(payload["requested_voltage_v"]), 0.0)
```

- [ ] **Step 2: Run the storage tests and verify the module is missing**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_storage -v
```

Expected: FAIL because `storage.py` does not exist.

- [ ] **Step 3: Implement atomic writers and directory naming**

Use same-directory temporary files opened as binary handles, then `os.replace(temp_path, final_path)`. Never pass an extension-less temporary pathname directly to `np.savez_compressed`, because NumPy may silently append `.npz`. Directory names use stable indices for identity and readable values for inspection:

```text
<run_id>/op_000_T22.000_I80.000/attempt_001/forward/000_V0.000000.npz
<run_id>/op_000_T22.000_I80.000/attempt_001/reverse/000_V12.934175.npz
```

Each NPZ stores raw arrays plus requested voltage, all eight measured voltage/current telemetry values, direction/index, acquisition timestamp, OSA duration, trace, identity, target/measured temperature, TEC state, current setpoint, and current-output state. Append the same scalar metadata to `acquisitions.csv` only after the NPZ replacement succeeds.

- [ ] **Step 4: Add failing tests for attempts, failures, and resume boundaries**

```python
def test_incomplete_attempt_is_never_selected_as_complete(self):
    run = make_run_store()
    first = run.start_attempt(0)
    first.mark_failed(RuntimeError("OSA timeout"))
    second = run.start_attempt(0)
    self.assertEqual(first.attempt_number, 1)
    self.assertEqual(second.attempt_number, 2)
    self.assertFalse(run.operation_complete(0))

def test_completed_operation_is_skipped_but_partial_operation_restarts(self):
    run = make_run_store()
    complete_attempt_with_schedule(run.start_attempt(0))
    run.start_attempt(1).mark_failed(RuntimeError("interrupted"))
    self.assertEqual(run.pending_operation_indices(), (1,))
```

- [ ] **Step 5: Implement manifests and exact state transitions**

`run.json` and every `status.json` are atomically replaced JSON documents. Allowed attempt states are `running -> complete` or `running -> failed`; no transition out of a terminal state is allowed. `mark_complete()` verifies exactly the configured schedule, all referenced NPZ files, and a single wavelength grid before setting complete. `open_existing()` never trusts a stale `status.json` alone; it revalidates referenced files.

- [ ] **Step 6: Run storage tests**

Run the Task 2 unittest command. Expected: all tests PASS.

- [ ] **Step 7: Commit if Git is available**

Commit message: `feat: add crash-safe SIL spectrum storage`.

---

### Task 3: Paired Forward/Reverse Analysis Without Scientific Overclaiming

**Files:**
- Create: `Code/Experiments/sil_hysteresis/analysis.py`
- Test: `Code/Debugs/test_sil_analysis_figure.py`

**Interfaces:**
- Consumes: `CompletedAttemptData` and `AnalysisConfig`.
- Produces: `ComparisonResult` with ascending comparable voltages, wavelength grid, forward matrix, aligned reverse matrix, signed difference matrix, per-voltage metrics, summary metrics, signal mask, excluded-count record, and exploratory classification.
- Produces: `compare_attempt(data, config) -> ComparisonResult` and `write_metrics_csv(result, path)`.

- [ ] **Step 1: Write failing paired-alignment tests**

```python
def test_reverse_branch_is_aligned_to_ascending_voltage(self):
    data = synthetic_attempt(points=4, reverse_offset_db=2.0)
    result = compare_attempt(data, AnalysisConfig(signal_window_db=30.0))
    np.testing.assert_allclose(result.voltage_v, data.voltage_grid[:-1])
    np.testing.assert_allclose(result.difference_db, -2.0)
    self.assertEqual(result.excluded_turning_points, 1)

def test_wavelength_mismatch_is_rejected_before_pairing(self):
    data = synthetic_attempt(points=4)
    data.reverse_wavelength_nm[1, 1] += 0.01
    with self.assertRaisesRegex(ValueError, "wavelength grid"):
        compare_attempt(data, AnalysisConfig())
```

- [ ] **Step 2: Run the analysis test class and verify failure**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_analysis_figure.AnalysisTests -v
```

Expected: FAIL because `analysis.py` does not exist.

- [ ] **Step 3: Implement exact metrics and data-integrity records**

For each comparable voltage, compute on raw dBm values:

```python
difference = forward_dbm - reverse_dbm
mae_db = np.mean(np.abs(difference))
rms_db = np.sqrt(np.mean(difference**2))
correlation = np.corrcoef(forward_dbm, reverse_dbm)[0, 1]
peak_shift_nm = wavelength_nm[np.argmax(forward_dbm)] - wavelength_nm[np.argmax(reverse_dbm)]
```

Also compute signal-region MAE/RMS/correlation using the union mask where either branch lies within `signal_window_db` of its own peak. Record the full wavelength-bin count, signal-mask count, exact mask rule, and the one excluded turning point. Summary fields are median and 95th percentile for absolute differences, median correlation, maximum absolute peak shift, and the five largest signal-region-MAE voltage points.

If `thresholds is None`, classification is exactly `OBSERVATION_REQUIRED`. If thresholds are supplied, validate and apply named keys `high_max_p95_mae_db`, `high_min_median_correlation`, `different_min_p95_mae_db`, and `different_max_median_correlation`; return `HIGH_CONSISTENCY`, `VISIBLE_DIFFERENCE`, or `REVIEW_RECOMMENDED`. Label every non-null result `exploratory`, not `pass` or `fail`.

- [ ] **Step 4: Add tests for noise masks, constants, and optional thresholds**

Test that constant spectra produce `NaN` correlation without crashing, an empty signal mask raises an explicit error, full-spectrum metrics retain every wavelength bin, and null thresholds always return `OBSERVATION_REQUIRED` even for identical synthetic branches.

- [ ] **Step 5: Implement CSV source-data output**

`metrics.csv` has one row per comparable voltage and explicit columns for voltage, voltage squared, all full/signal metrics, bin counts, and peak positions. `summary.json` includes aggregation definitions, classification state, threshold mapping or null, and discrepancy ranking. Do not round values in stored source data; formatting belongs only in logs and figures.

- [ ] **Step 6: Run analysis tests**

Run the Task 3 unittest command. Expected: all analysis tests PASS; figure tests may still fail because `figure.py` is not implemented.

- [ ] **Step 7: Commit if Git is available**

Commit message: `feat: add paired SIL hysteresis analysis`.

---

### Task 4: Nature-Style Static Figure and Reusable Latest-Result Window

**Files:**
- Create: `Code/Experiments/sil_hysteresis/figure.py`
- Modify: `Code/Debugs/test_sil_analysis_figure.py`

**Interfaces:**
- Consumes: `ComparisonResult`, operation-point metadata, and output base path.
- Produces: `build_comparison_figure(result, metadata) -> matplotlib.figure.Figure`, `save_comparison_figure(fig, base_path) -> tuple[Path, ...]`, and `LatestResultWindow.update(result, metadata)`.

- [ ] **Step 1: Write failing export and window-reuse tests**

```python
class FigureTests(unittest.TestCase):
    def test_exports_editable_primary_and_preview_formats(self):
        fig = build_comparison_figure(comparison_result(), metadata())
        paths = save_comparison_figure(fig, self.output / "comparison")
        self.assertEqual({p.suffix for p in paths}, {".svg", ".pdf", ".png"})
        self.assertIn("<text", (self.output / "comparison.svg").read_text(encoding="utf-8"))

    def test_latest_window_reuses_one_figure(self):
        window = LatestResultWindow(pyplot=fake_pyplot())
        window.update(comparison_result(), metadata(operation_index=0))
        first_identity = id(window.figure)
        window.update(comparison_result(), metadata(operation_index=1))
        self.assertEqual(id(window.figure), first_identity)
        self.assertEqual(window.pyplot.show_calls, [False])
```

- [ ] **Step 2: Run the figure tests and verify failure**

Run the full `Code.Debugs.test_sil_analysis_figure` module. Expected: FAIL because the figure API is missing.

- [ ] **Step 3: Implement the approved figure contract**

Before creating a figure, set the mandatory editable-text parameters:

```python
plt.rcParams["font.family"] = "sans-serif"
plt.rcParams["font.sans-serif"] = ["Arial", "DejaVu Sans", "Liberation Sans"]
plt.rcParams["svg.fonttype"] = "none"
plt.rcParams["pdf.fonttype"] = 42
```

Use a 180 mm-wide quantitative grid with small bold lowercase panel labels. Panels `a` and `b` are forward/reverse spectrograms with identical wavelength limits, V² limits, and power color normalization. Panel `c` is the visually dominant signed difference heatmap with a diverging color scale centered exactly at zero and symmetric limits derived from the 99th percentile absolute difference, while retaining all raw values in source data. Panel `d` plots signal-region MAE and absolute peak shift against V² and directly annotates the largest discrepancy voltage. Use white backgrounds, restrained blue/neutral accents, and no green/red pass/fail encoding.

The title states temperature/current and the exploratory classification. A footnote states the turning-point exclusion and mask rule. Do not claim locking quality.

- [ ] **Step 4: Implement exports and single-window display**

Save `comparison.svg`, `comparison.pdf`, and `comparison.png` at 300 dpi, then close the saved figure when no display is requested. `LatestResultWindow` creates one non-blocking window on its first update with `plt.show(block=False)`; later updates call `figure.clear()`, rebuild panels in the same figure, `canvas.draw_idle()`, and `canvas.flush_events()`. Catch GUI-backend errors, disable later display attempts, and log one warning without affecting saved files or experiment state.

- [ ] **Step 5: Run tests and static figure-source validation**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_analysis_figure -v
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' (Join-Path $env:USERPROFILE '.codex\skills\nature-figure\scripts\validate_figure.py') Code\Experiments\sil_hysteresis\figure.py
```

Expected: all tests PASS and the validator reports no blocking issue.

- [ ] **Step 6: Commit if Git is available**

Commit message: `feat: add Nature-style SIL comparison figure`.

---

### Task 5: Synthetic Similar and Hysteretic Instrument Bundle

**Files:**
- Create: `Code/Experiments/sil_hysteresis/simulate.py`
- Test: `Code/Debugs/test_sil_experiment_cli.py`

**Interfaces:**
- Produces: `SimulatedInstrumentBundle(mode: Literal["similar", "hysteretic"], seed: int = 0, clock=time.monotonic, sleep=time.sleep)` with context-compatible `osa`, `voltage`, and `gain` objects implementing only the public methods consumed by `ExperimentRunner`.
- The simulator never imports or constructs real drivers.

- [ ] **Step 1: Write failing tests for deterministic synthetic spectra and no real-driver construction**

```python
def test_similar_mode_returns_same_branch_at_matching_voltage(self):
    bundle = SimulatedInstrumentBundle(mode="similar", seed=7)
    forward = bundle.osa.spectrum_for(6.5, "forward")
    reverse = bundle.osa.spectrum_for(6.5, "reverse")
    np.testing.assert_allclose(forward.power_dbm, reverse.power_dbm, atol=0.25)

def test_hysteretic_mode_shifts_peak_on_reverse_branch(self):
    bundle = SimulatedInstrumentBundle(mode="hysteretic", seed=7)
    forward = bundle.osa.spectrum_for(6.5, "forward")
    reverse = bundle.osa.spectrum_for(6.5, "reverse")
    self.assertGreater(abs(peak_nm(forward) - peak_nm(reverse)), 0.01)
```

- [ ] **Step 2: Run the targeted tests and verify failure**

Run `python -m unittest Code.Debugs.test_sil_experiment_cli.SimulationTests -v` with the VISA interpreter. Expected: FAIL because `simulate.py` does not exist.

- [ ] **Step 3: Implement interface-compatible simulated devices**

Use a fixed wavelength grid and a Gaussian main peak whose center moves smoothly with V². In `similar` mode, both directions share the same deterministic center and power with only seeded low-amplitude repeatability noise. In `hysteretic` mode, reverse spectra receive a smooth voltage-dependent center offset that is zero at the 13 V turning point and largest mid-scan. Simulated voltage telemetry follows the commanded eight-channel values; simulated Gain status enforces the same setup ordering in its event log.

- [ ] **Step 4: Test cleanup and event ordering in simulation**

Assert the event log contains temperature set, TEC enable, stability wait, current set, current enable, voltage steps, emergency zero, current disable, and TEC disable in the approved order. Assert `--simulate` paths never call patched `AQ6370`, `VoltageSource`, or `GainDriver` constructors.

- [ ] **Step 5: Run simulation tests**

Expected: all `SimulationTests` PASS.

- [ ] **Step 6: Commit if Git is available**

Commit message: `test: add deterministic SIL experiment simulator`.

---

### Task 6: Fail-Fast Experiment Orchestration and Guaranteed Shutdown

**Files:**
- Create: `Code/Experiments/sil_hysteresis/experiment.py`
- Modify: `Code/Debugs/test_sil_experiment_cli.py`

**Interfaces:**
- Consumes: `RunConfig`, schedule builders, `RunStore`, analysis/figure functions, and an injected instrument bundle factory.
- Produces: `ExperimentRunner.run() -> RunSummary`, `ExperimentRunner.run_operation(index, point) -> OperationSummary`, and `safe_shutdown(voltage, gain, osa, log)`.

- [ ] **Step 1: Write failing orchestration tests with fake instruments**

```python
def test_one_operation_runs_199_acquisitions_and_saves_each_before_next_voltage(self):
    runner, events = make_runner(points=100)
    summary = runner.run()
    self.assertEqual(summary.acquisitions, 199)
    self.assert_precedes_every(events, "store.save", "voltage.next")

def test_osa_failure_stops_scan_and_zeros_before_gain_shutdown(self):
    runner, events = make_runner(osa_failure_at=12)
    with self.assertRaisesRegex(RuntimeError, "OSA failed"):
        runner.run()
    self.assertIn("attempt.failed", events)
    self.assertLess(events.index("voltage.emergency_zero"), events.index("gain.disable_current"))
    self.assertLess(events.index("gain.disable_current"), events.index("gain.disable_tec"))
    self.assertNotIn("osa.acquire.13", events)
```

- [ ] **Step 2: Run orchestration tests and verify failure**

Run the `ExperimentRunnerTests` class. Expected: FAIL because `experiment.py` does not exist.

- [ ] **Step 3: Implement the exact lifecycle**

For each pending operation point:

```text
emergency-zero voltage
disable Gain current if enabled
set target temperature
enable TEC if disabled
wait_stable(temperature_timeout_s)
set current
enable_current
sleep(ld_settle_s)
for each ScanStep:
    set_channel(channel, voltage)
    wait for fresh voltage telemetry
    sleep(settle_time_s)
    time osa.acquire(trace)
    read fresh Gain status/temperature
    atomically save acquisition
    update timing tracker and log ETA
emergency-zero voltage
disable Gain current
mark attempt complete
analyze, save metrics and figure
update latest-result window
```

Connect instruments only after the CLI has confirmed the plan. Use one outer `try/finally` that calls `safe_shutdown` even for `KeyboardInterrupt` and `BaseException`. `safe_shutdown` attempts every step independently in this order: voltage emergency zero, Gain current disable, Gain TEC disable/close, VoltageSource close, OSA close. It logs cleanup failures without replacing an active primary exception; if no primary exception exists, surface the first cleanup failure after all cleanup steps are attempted.

Connection order is OSA identity first, Voltage Source second so startup zero is confirmed, and Gain Driver third for its read-only status snapshot. Immediately after Gain connection, normalize the experiment baseline by emergency-zeroing voltage and disabling Gain current before changing a temperature or current setpoint. Do not rely on context-manager reverse ordering for safety; the explicit outer `finally` owns the required cross-instrument shutdown order, while each driver's idempotent `close()` remains the resource-release backstop.

- [ ] **Step 4: Add tests for operation-point transitions and completed-point skipping**

Verify current is disabled before every temperature or current change, TEC may remain enabled between successfully completed operation points, completed operation points are skipped on resume, incomplete points start a fresh attempt at 0 V, and final shutdown disables TEC. Verify analysis or figure failure also triggers the same safe shutdown and never marks an attempt complete unless acquisition validation already passed.

If a complete acquisition attempt lacks `metrics.csv`, `summary.json`, or figure files because post-processing failed, resume regenerates those products from stored spectra before deciding that the operation point needs hardware acquisition. It must not reacquire a complete triangular scan merely because plotting failed.

- [ ] **Step 5: Run orchestration tests**

Run the full `Code.Debugs.test_sil_experiment_cli` module. Expected: orchestration and simulation tests PASS; CLI tests may still fail until Task 7.

- [ ] **Step 6: Commit if Git is available**

Commit message: `feat: orchestrate safe SIL hysteresis scans`.

---

### Task 7: CLI Planning, Confirmation, Logging, Resume, and Timing Output

**Files:**
- Create: `Code/Experiments/sil_hysteresis/run.py`
- Modify: `Code/Experiments/sil_hysteresis/__init__.py`
- Modify: `Code/Debugs/test_sil_experiment_cli.py`

**Interfaces:**
- Produces CLI modes: normal real run, `--plan`, `--simulate similar`, `--simulate hysteretic`, and `--resume RUN_DIRECTORY`.
- Real run constructs public drivers only after exact confirmation text is received.

- [ ] **Step 1: Write failing tests for plan-only and confirmation cancellation**

```python
def test_plan_mode_never_constructs_instruments(self):
    result = main(["--config", str(DEFAULT_CONFIG), "--plan"], factories=forbidden_factories())
    self.assertEqual(result, 0)

def test_declined_confirmation_never_constructs_instruments(self):
    result = main(["--config", str(DEFAULT_CONFIG)], input_fn=lambda _: "NO", factories=forbidden_factories())
    self.assertEqual(result, 0)

def test_real_run_requires_exact_run_confirmation(self):
    for answer in ("y", "yes", "run ", ""):
        self.assertEqual(main(real_args(), input_fn=lambda _: answer, factories=forbidden_factories()), 0)
```

- [ ] **Step 2: Run CLI tests and verify failure**

Run `python -m unittest Code.Debugs.test_sil_experiment_cli.CLITests -v` with the VISA interpreter. Expected: FAIL because `run.py` does not exist.

- [ ] **Step 3: Implement argument parsing and complete plan display**

Arguments are:

```text
--config PATH                  default: package default_config.json
--plan                         validate and print only; construct no instruments
--simulate {similar,hysteretic}
--resume RUN_DIRECTORY
--no-display                   save figures without latest-result window
--log-level {DEBUG,INFO,WARNING,ERROR}
```

Before confirmation, print every operation point, voltage channel/range/spacing/point count, 199 acquisitions per operation point, total acquisitions, `settle_time_s`, minimum cumulative voltage-ramp time, configured non-OSA waits, OSA time as unknown, output directory, and exact shutdown behavior. Require the exact token `RUN` for real hardware. Simulation does not require hardware confirmation and labels every output `SIMULATED`.

- [ ] **Step 4: Implement structured terminal and file logging**

Use one timestamped log format and handlers for stdout plus `<run>/experiment.log`. Log one INFO line per acquisition containing operation index, direction, point index, requested/measured voltage, measured temperature, OSA duration, completed/total count, mean OSA duration, and ETA. Never log spectrum arrays. On each completed operation, log figure paths, summary metrics, discrepancy voltages, and classification.

- [ ] **Step 5: Implement real-driver factories without direct transports**

Construct only through keyword dictionaries that preserve the Gain Driver's default USB serial preference:

```python
osa = AQ6370(resource_name=config.devices.osa_resource)
voltage = VoltageSource(port=config.devices.voltage_port)
gain_kwargs = {"port": config.devices.gain_port}
if config.devices.gain_serial_number is not None:
    gain_kwargs["usb_serial"] = config.devices.gain_serial_number
gain = GainDriver(**gain_kwargs)
```

Do not access `_resource`, `_serial`, or any other private driver member. The runner calls their public context/lifecycle and operation methods only.

- [ ] **Step 6: Run all new offline tests**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_config_scan Code.Debugs.test_sil_storage Code.Debugs.test_sil_analysis_figure Code.Debugs.test_sil_experiment_cli -v
```

Expected: all new tests PASS and no serial/VISA resource is constructed.

- [ ] **Step 7: Commit if Git is available**

Commit message: `feat: add SIL hysteresis experiment CLI`.

---

### Task 8: End-to-End Simulation, Figure QA, and Documentation

**Files:**
- Modify: `Code/Experiments/sil_hysteresis/default_config.json` only if simulation exposes a validated default error
- Create: `Code/Experiments/sil_hysteresis/README.md`
- Generated during verification: `Result/sil_hysteresis/<simulation-run>/...`

**Interfaces:**
- Documents exact user commands, output layout, resume rule, metric interpretation, and hardware warning.
- Produces verified similar and hysteretic simulation run artifacts.

- [ ] **Step 1: Run both complete simulations with the approved default config**

Run `--simulate similar --no-display` and `--simulate hysteretic --no-display` using `default_config.json`. The simulator uses an injected no-op sleep, so the full 199-acquisition schedule completes quickly without weakening coverage. Expected: 199 spectra per run, complete manifests, SVG/PDF/PNG figures, `metrics.csv`, `summary.json`, and no hardware construction.

- [ ] **Step 2: Verify scientific behavior of synthetic results**

Assert the similar simulation has smaller 95th-percentile signal MAE and peak shift than the hysteretic simulation. Confirm both remain `OBSERVATION_REQUIRED` while thresholds are null. Confirm every displayed data point maps to stored source data and the shared turning point is absent from paired metrics.

- [ ] **Step 3: Perform nature-figure delivery QA**

Before QA, re-read the installed `nature-figure` `references/qa-contract.md`. Run its source validator on `figure.py`, run `audit_pdf_text.py --min-pt 5` on both PDFs, render/open the PNGs, and inspect every panel at final physical size for shared color limits, label collisions, readable colorbars, difference-map centering, visible panel hierarchy, and correct exploratory wording. Fix any failure and rerun the complete figure QA.

- [ ] **Step 4: Write the experiment README**

Document:

- offline `--plan` and `--simulate` commands;
- approved defaults and how to reduce `points`;
- explicit-list and temperature-current-grid JSON examples;
- real-hardware `RUN` confirmation;
- 199-acquisition timing model and rolling ETA;
- output tree and per-spectrum NPZ fields;
- why incomplete scans restart from 0 V;
- how to use `--resume` to skip complete operation points;
- the meaning of `OBSERVATION_REQUIRED` and why it is not a locking-quality verdict;
- latest-window behavior and headless fallback;
- fail-fast and shutdown order.

- [ ] **Step 5: Run legacy driver tests plus all experiment tests**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_drivers Code.Debugs.test_sil_config_scan Code.Debugs.test_sil_storage Code.Debugs.test_sil_analysis_figure Code.Debugs.test_sil_experiment_cli -v
```

Expected: all tests PASS. A failure in legacy driver tests blocks hardware validation.

- [ ] **Step 6: Commit if Git is available**

Commit message: `docs: verify and document SIL hysteresis automation`.

---

### Task 9: Operator-Gated Hardware Validation — Do Not Run Without New Confirmation

**Files:**
- No code changes unless a verified hardware incompatibility is found.
- Runtime outputs: `Result/sil_hysteresis/<hardware-run>/...`

**Interfaces:**
- Uses existing `Code/Debugs/check_osa.py`, `check_voltage.py`, `check_gain.py`, and `check_all.py` before the experiment CLI.
- This task is an operational checklist, not authorization to touch hardware.

- [ ] **Step 1: Present the exact state-changing hardware plan and obtain fresh user confirmation**

Disclose that Gain TEC will target 22 °C, laser current will be set to 80 mA and enabled only after stability, Voltage Source CH1 will scan to 13 V, 199 OSA sweeps will occur, and handled failure will request immediate voltage zero followed by current/TEC shutdown. Stop until the user explicitly approves this live run.

- [ ] **Step 2: Run identity/read-only checks first**

After approval, run the existing OSA identity/acquisition check and read-only portions of voltage/Gain checks in the VISA environment. Do not use direct serial/VISA commands. Resolve any device resource/port configuration from observed identities rather than guessing.

- [ ] **Step 3: Run existing operator-confirmed minimal output checks**

Use `Code.Debugs.check_all` and its explicit prompts to confirm startup zero, CH1 0.1 V behavior, Gain TEC stability, controlled current enable/disable, and resource cleanup before the full experiment.

- [ ] **Step 4: Print the full 100-point experiment plan without touching hardware**

Run the experiment CLI with `--plan`. Review the displayed 22 °C / 80 mA point, CH1 0–13 V range, V² spacing, 100 points, 0.1 s settle, 199 acquisitions, resource addresses, output directory, and timing lower bound.

- [ ] **Step 5: Run the approved single-point experiment**

Start the real CLI, type the exact `RUN` token, monitor temperature/current/voltage telemetry and rolling ETA, and do not manually modify OSA front-panel settings during acquisition. Verify the latest-result window appears only after voltage zero and current disable.

- [ ] **Step 6: Inspect shutdown and output completeness before any batch expansion**

Confirm measured voltage returns below the driver's zero threshold, Gain current and TEC are disabled on final close, 199 spectrum files exist, the attempt is complete, figures and metrics open correctly, and logs contain no cleanup warning. Only after this single point passes should a later user-approved change add multiple operation points.

## Final Self-Review Checklist

- [ ] Every approved design requirement maps to a task above.
- [ ] No experiment module opens serial or VISA transports directly.
- [ ] All output-changing operations remain inside existing driver limits and interlocks.
- [ ] The plan distinguishes a complete operation point from an incomplete attempt and forbids mid-scan continuation.
- [ ] Raw data, metric masks, exclusions, figures, and logs remain traceable.
- [ ] Interactive plotting cannot block while laser current remains enabled.
- [ ] The no-threshold result cannot be mistaken for a scientific pass/fail verdict.
- [ ] Hardware validation remains explicitly unauthorized until Task 9 receives fresh confirmation.
