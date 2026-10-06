# SIL Repeated-Cycle Scan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a safe, resumable repeated SIL experiment that holds 24.0 °C and 100.0 mA while acquiring 100 forward/reverse cycles with 40 V²-uniform observations per branch from 40 to 90 V².

**Architecture:** Add an isolated `Code/Experiments/sil_repeat` package so the proven single-triangle experiment and schema remain unchanged. The new package uses a bounded manifest per 80-spectrum cycle, reuses the existing numerical comparison/figure code through an explicit adapter, queues completed cycles to a postprocessing process, and keeps the hardware acquisition loop independent of figure export.

**Tech Stack:** Python 3 in the Anaconda `VISA` environment, `unittest`, NumPy, Matplotlib, multiprocessing, existing `Code/Utils` drivers, atomic JSON/CSV/NPZ persistence.

**Spec:** `docs/superpowers/specs/2026-08-21-sil-repeat-scan-design.md`

## Global Constraints

- Run every Python command with `D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe`.
- Experiment code may construct instruments only through `Code/Utils`; it must not open serial or VISA resources directly.
- Commanded Voltage Source values remain in 0–14 V; normal motion is delegated to the driver and therefore uses steps no larger than 0.1 V with at least 50 ms between steps.
- Gain commands remain in 0–200 mA and 15–40 °C; current is enabled only after the driver's five-second ±0.2 °C TEC interlock.
- Shutdown order is immediate all-channel voltage zero, Gain current disable, Gain TEC disable, Gain close, Voltage Source close, OSA close.
- OSA acquisition preserves all front-panel measurement settings.
- The approved schedule is exactly 100 cycles × 2 branches × 40 observations = 8,000 spectra at 24.0 °C and 100.0 mA.
- Use V² coordinates `linspace(40.0, 90.0, 40)` and command `sqrt(V²)`, approximately 6.3246–9.4868 V.
- Store all 40 observations from both branches; exclude the duplicated high-turning pair from the primary 39-pair hysteresis aggregate.
- Keep user-owned changes to `Code/Experiments/sil_hysteresis/grid_4x4.json` and `Code/Experiments/sil_hysteresis/single_24C_100mA_12V.json` out of feature commits.
- Write generated data only below `Result/sil_repeat` on the workspace drive.

---

### Task 1: Validated repeat configuration and exact schedule

**Files:**
- Create: `Code/Experiments/sil_repeat/__init__.py`
- Create: `Code/Experiments/sil_repeat/config.py`
- Create: `Code/Experiments/sil_repeat/scan.py`
- Create: `Code/Debugs/test_sil_repeat_config_scan.py`

**Interfaces:**
- Consumes: `DeviceConfig`, `OperationPoint`, and `AnalysisConfig` from `Code.Experiments.sil_hysteresis.config`.
- Produces: `RepeatScanConfig`, `RepeatRunConfig`, `load_repeat_config(path: Path) -> RepeatRunConfig`, `RepeatStep`, `build_v2_grid(scan: RepeatScanConfig) -> np.ndarray`, `build_repeat_schedule(scan: RepeatScanConfig) -> Sequence[RepeatStep]`, and `cycle_steps(schedule, cycle_index) -> Sequence[RepeatStep]`.

- [ ] **Step 1: Write failing configuration and schedule tests**

```python
class RepeatScheduleTests(unittest.TestCase):
    def test_approved_schedule_has_exact_branch_counts_and_endpoints(self):
        scan = RepeatScanConfig(v2_min=40.0, v2_max=90.0, points_per_branch=40, cycles=100)
        steps = build_repeat_schedule(scan)
        self.assertEqual(len(steps), 8000)
        self.assertEqual([s.direction for s in steps[:40]], ["forward"] * 40)
        self.assertEqual([s.direction for s in steps[40:80]], ["reverse"] * 40)
        self.assertEqual(steps[0].voltage_squared_v2, 40.0)
        self.assertEqual(steps[39].voltage_squared_v2, 90.0)
        self.assertEqual(steps[40].voltage_squared_v2, 90.0)
        self.assertEqual(steps[79].voltage_squared_v2, 40.0)
        self.assertEqual(steps[0].cycle_index, 0)
        self.assertEqual(steps[-1].cycle_index, 99)
        self.assertEqual([s.sequence_index for s in steps], list(range(8000)))

    def test_v2_grid_is_uniform_and_voltage_is_square_root(self):
        scan = RepeatScanConfig(v2_min=40.0, v2_max=90.0, points_per_branch=40, cycles=1)
        grid = build_v2_grid(scan)
        np.testing.assert_allclose(grid, np.linspace(40.0, 90.0, 40))
        steps = build_repeat_schedule(scan)
        self.assertAlmostEqual(steps[0].voltage_v, math.sqrt(40.0))
        self.assertAlmostEqual(steps[39].voltage_v, math.sqrt(90.0))

    def test_repeat_config_rejects_invalid_counts_and_voltage_squared_limits(self):
        for kwargs in (
            {"cycles": 0},
            {"cycles": True},
            {"points_per_branch": 1},
            {"v2_min": -0.1},
            {"v2_max": 196.1},
            {"v2_min": 90.0, "v2_max": 40.0},
        ):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                RepeatScanConfig(**kwargs)
```

- [ ] **Step 2: Run the tests and verify the missing package/API failure**

Run:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_config_scan -v
```

Expected: import failure for `Code.Experiments.sil_repeat`.

- [ ] **Step 3: Implement immutable configuration and scheduling**

```python
@dataclass(frozen=True)
class RepeatScanConfig:
    channel: int = 1
    v2_min: float = 40.0
    v2_max: float = 90.0
    points_per_branch: int = 40
    cycles: int = 100
    settle_time_s: float = 0.1

    def __post_init__(self) -> None:
        if isinstance(self.channel, bool) or not isinstance(self.channel, int) or not 1 <= self.channel <= 8:
            raise ValueError("channel must be an integer between 1 and 8")
        if isinstance(self.points_per_branch, bool) or not isinstance(self.points_per_branch, int) or self.points_per_branch < 2:
            raise ValueError("points_per_branch must be an integer of at least two")
        if isinstance(self.cycles, bool) or not isinstance(self.cycles, int) or self.cycles < 1:
            raise ValueError("cycles must be a positive integer")
        checked = {}
        for name, value in (("v2_min", self.v2_min), ("v2_max", self.v2_max), ("settle_time_s", self.settle_time_s)):
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(float(value)):
                raise ValueError(f"{name} must be finite")
            checked[name] = float(value)
        if not 0.0 <= checked["v2_min"] < checked["v2_max"] <= 196.0:
            raise ValueError("V² bounds must satisfy 0 <= v2_min < v2_max <= 196")
        if checked["settle_time_s"] < 0.0:
            raise ValueError("settle_time_s must be non-negative")
        object.__setattr__(self, "v2_min", checked["v2_min"])
        object.__setattr__(self, "v2_max", checked["v2_max"])
        object.__setattr__(self, "settle_time_s", checked["settle_time_s"])


@dataclass(frozen=True)
class RepeatStep:
    sequence_index: int
    cycle_index: int
    direction: str
    branch_index: int
    pair_index: int
    voltage_squared_v2: float
    voltage_v: float


def build_repeat_schedule(scan: RepeatScanConfig) -> Sequence[RepeatStep]:
    v2_grid = build_v2_grid(scan)
    steps: list[RepeatStep] = []
    for cycle_index in range(scan.cycles):
        branches = (("forward", v2_grid), ("reverse", v2_grid[::-1]))
        for direction, branch in branches:
            for branch_index, v2 in enumerate(branch):
                pair_index = branch_index if direction == "forward" else scan.points_per_branch - 1 - branch_index
                steps.append(RepeatStep(len(steps), cycle_index, direction, branch_index, pair_index, float(v2), math.sqrt(float(v2))))
    return tuple(steps)
```

Replace the validation comment and ellipsis with explicit checks using the established helpers/patterns from `sil_hysteresis.config`; no permissive coercion of booleans or strings.

- [ ] **Step 4: Run the focused tests and confirm they pass**

Run the Step 2 command. Expected: all tests pass.

- [ ] **Step 5: Commit the schedule slice**

```powershell
git add -- Code/Experiments/sil_repeat/__init__.py Code/Experiments/sil_repeat/config.py Code/Experiments/sil_repeat/scan.py Code/Debugs/test_sil_repeat_config_scan.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: define repeated SIL scan schedule"
```

### Task 2: Bounded per-cycle atomic storage and resume validation

**Files:**
- Create: `Code/Experiments/sil_repeat/storage.py`
- Create: `Code/Debugs/test_sil_repeat_storage.py`

**Interfaces:**
- Consumes: `RepeatRunConfig`, `RepeatStep`, `cycle_steps`, driver `Spectrum`, `VoltageStatus`, and `GainStatus` snapshots.
- Produces: `RepeatRunStore.create`, `RepeatRunStore.open_existing`, `RepeatRunStore.start_cycle_attempt`, `RepeatRunStore.pending_cycle_indices`, `CycleAttemptStore.save_acquisition`, `CycleAttemptStore.mark_acquisition_complete`, `CycleAttemptStore.mark_failed`, `CycleData`, and `load_complete_cycle(path: Path) -> CycleData`.

- [ ] **Step 1: Write failing storage behavior tests**

```python
class RepeatStorageTests(unittest.TestCase):
    def test_each_cycle_has_its_own_bounded_manifest_and_unique_branch_paths(self):
        with tempfile.TemporaryDirectory() as root:
            config = make_repeat_config(Path(root), cycles=2, points_per_branch=2)
            run = RepeatRunStore.create(config, run_mode="similar")
            attempt = run.start_cycle_attempt(0)
            for step in cycle_steps(run.schedule, 0):
                attempt.save_acquisition(step=step, spectrum=fake_spectrum(), voltage_status=fake_voltage_status(), gain_status=fake_gain_status(), osa_duration_s=1.25)
            attempt.mark_acquisition_complete()
            status = json.loads((attempt.path / "status.json").read_text(encoding="utf-8"))
            self.assertEqual(status["state"], "acquired")
            self.assertEqual(len(status["acquisitions"]), 4)
            self.assertTrue((attempt.path / "forward" / "000_V2_40.000000.npz").is_file())
            self.assertTrue((attempt.path / "reverse" / "000_V2_90.000000.npz").is_file())
            self.assertEqual(run.pending_cycle_indices(), (1,))

    def test_partial_cycle_is_preserved_but_resume_creates_new_attempt(self):
        with tempfile.TemporaryDirectory() as root:
            run = RepeatRunStore.create(make_repeat_config(Path(root), cycles=1, points_per_branch=2))
            first = run.start_cycle_attempt(0)
            first.save_acquisition(step=cycle_steps(run.schedule, 0)[0], spectrum=fake_spectrum(), voltage_status=fake_voltage_status(), gain_status=fake_gain_status(), osa_duration_s=1.0)
            first.mark_failed(RuntimeError("OSA timeout"))
            reopened = RepeatRunStore.open_existing(run.path)
            second = reopened.start_cycle_attempt(0)
            self.assertEqual(second.attempt_number, 2)
            self.assertTrue(first.path.joinpath("forward").exists())
```

- [ ] **Step 2: Run and verify the storage import/API failures**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_storage -v
```

Expected: failure because `sil_repeat.storage` does not exist.

- [ ] **Step 3: Implement cycle-scoped atomic persistence**

Use this on-disk identity and state machine:

```python
@dataclass(frozen=True)
class CycleData:
    path: Path
    cycle_index: int
    attempt_number: int
    acquisitions: Sequence[RepeatStoredAcquisition]
    run_mode: str
```

Implement `CycleAttemptStore.save_acquisition(step: RepeatStep, spectrum: Spectrum, voltage_status: VoltageStatus, gain_status: GainStatus, osa_duration_s: float) -> RepeatStoredAcquisition`, `mark_acquisition_complete() -> None`, `mark_postprocessed() -> None`, and `mark_failed(error: BaseException) -> None` with the exact `running -> acquired -> complete` and `running -> failed` transitions. Implement local `_write_json_atomic` and `_write_npz_atomic` with a temporary file in the destination directory followed by `Path.replace`. Store `cycle_index`, `direction`, `branch_index`, `pair_index`, `voltage_squared_v2`, `requested_voltage_v`, all eight voltage/current telemetry channels, Gain state, OSA audit fields, and provenance in both the NPZ and scalar manifest row. Validate every referenced NPZ before treating `acquired` or `complete` as reusable. Keep each status manifest bounded to one cycle's 80 records.

- [ ] **Step 4: Add corruption and duplicate-step tests, then run green**

Add tests that delete one referenced NPZ, alter one sequence index, and attempt to store the same step twice. Each must raise `ValueError`, and `pending_cycle_indices()` must include the corrupted cycle. Run the Step 2 command. Expected: all tests pass.

- [ ] **Step 5: Commit the storage slice**

```powershell
git add -- Code/Experiments/sil_repeat/storage.py Code/Debugs/test_sil_repeat_storage.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: persist repeated scans by cycle"
```

### Task 3: Per-cycle comparison adapter and aggregate trend products

**Files:**
- Create: `Code/Experiments/sil_repeat/analysis.py`
- Create: `Code/Experiments/sil_repeat/figure.py`
- Create: `Code/Debugs/test_sil_repeat_analysis_figure.py`

**Interfaces:**
- Consumes: `CycleData`, `AnalysisConfig`, and the public result/writer/figure interfaces from `sil_hysteresis.analysis` and `sil_hysteresis.figure`.
- Produces: `compare_cycle(data, config) -> ComparisonResult`, `CycleSummaryRow`, `write_cycle_products`, `append_repeat_summary`, and `write_repeat_summary_figure`.

- [ ] **Step 1: Write failing adapter tests that independently establish endpoint semantics**

```python
class RepeatAnalysisTests(unittest.TestCase):
    def test_compare_cycle_keeps_40_raw_per_branch_but_compares_39_pairs(self):
        data = completed_cycle_fixture(points_per_branch=40)
        result = compare_cycle(data, AnalysisConfig(signal_window_db=30.0))
        self.assertEqual(len([a for a in data.acquisitions if a.step.direction == "forward"]), 40)
        self.assertEqual(len([a for a in data.acquisitions if a.step.direction == "reverse"]), 40)
        self.assertEqual(result.voltage_v.shape, (39,))
        self.assertAlmostEqual(result.voltage_v[0] ** 2, 40.0)
        self.assertLess(result.voltage_v[-1] ** 2, 90.0)
        self.assertEqual(result.excluded_turning_points, 1)

    def test_repeat_summary_contains_one_row_per_cycle_in_cycle_order(self):
        with tempfile.TemporaryDirectory() as root:
            rows = (cycle_summary(1, 3.0, 0.2), cycle_summary(0, 2.0, 0.1))
            path = append_repeat_summary(Path(root), rows)
            saved = list(csv.DictReader(path.open(newline="", encoding="utf-8")))
            self.assertEqual([int(row["cycle_index"]) for row in saved], [0, 1])
```

- [ ] **Step 2: Run and verify failures before implementation**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_analysis_figure -v
```

Expected: import failure for the new analysis module.

- [ ] **Step 3: Implement the explicit adapter to proven single-cycle analysis**

```python
def compare_cycle(data: CycleData, config: AnalysisConfig) -> ComparisonResult:
    forward = sorted((a for a in data.acquisitions if a.step.direction == "forward"), key=lambda a: a.step.branch_index)
    reverse = sorted((a for a in data.acquisitions if a.step.direction == "reverse"), key=lambda a: a.step.branch_index)
    if len(forward) != len(reverse) or len(forward) < 2:
        raise ValueError("cycle must contain equal complete branches")
    adapted: list[StoredAcquisition] = []
    for acquisition in forward:
        comparison = None if acquisition.step.pair_index == len(forward) - 1 else acquisition.step.pair_index
        adapted.append(as_hysteresis_acquisition(acquisition, "forward", acquisition.step.branch_index, comparison))
    for acquisition in reverse:
        if acquisition.step.pair_index == len(forward) - 1:
            continue
        adapted.append(as_hysteresis_acquisition(acquisition, "reverse", acquisition.step.branch_index - 1, acquisition.step.pair_index))
    return compare_attempt(CompletedAttemptData(data.path, data.cycle_index, data.attempt_number, tuple(adapted), data.run_mode), config)
```

`as_hysteresis_acquisition` constructs the public `ScanStep` and `StoredAcquisition` dataclasses without copying spectra. Assert exact voltage/pair identity before adapting. `write_cycle_products` writes metrics/summary and the three comparison formats below the cycle attempt. Add a visible `Cycle n/100` annotation without changing the original `sil_hysteresis` figure implementation.

- [ ] **Step 4: Implement deterministic aggregate CSV and Nature-style trend figure**

Write `repeat_summary.csv` via a temporary replacement so reruns cannot duplicate rows. Plot cycle number on x, P95 signal MAE on the left axis, and maximum absolute peak shift on the right axis. Export SVG/PDF/300-dpi PNG with editable SVG/PDF text and REAL/SIMULATED provenance metadata.

- [ ] **Step 5: Run focused analysis/figure tests and visually render one fixture PNG**

Run the Step 2 command. Expected: all tests pass and the fixture produces finite metrics plus three non-empty figure files. Inspect the PNG with the local image viewer and correct clipping, labels, or unreadable scaling before continuing.

- [ ] **Step 6: Commit the analysis slice**

```powershell
git add -- Code/Experiments/sil_repeat/analysis.py Code/Experiments/sil_repeat/figure.py Code/Debugs/test_sil_repeat_analysis_figure.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: analyze repeated SIL cycles"
```

### Task 4: Non-blocking postprocessing process

**Files:**
- Create: `Code/Experiments/sil_repeat/postprocess.py`
- Create: `Code/Debugs/test_sil_repeat_postprocess.py`

**Interfaces:**
- Consumes: completed cycle attempt paths, frozen repeat config payload, `write_cycle_products`, and aggregate writers.
- Produces: `PostprocessRequest`, `InlinePostprocessor`, `ProcessPostprocessor.submit`, `ProcessPostprocessor.check`, and `ProcessPostprocessor.close_and_drain`.

- [ ] **Step 1: Write failing inline-worker contract tests**

```python
class PostprocessorTests(unittest.TestCase):
    def test_inline_worker_processes_cycles_in_submission_order_and_drains(self):
        calls = []
        worker = InlinePostprocessor(lambda request: calls.append(request.cycle_index))
        worker.submit(PostprocessRequest(0, Path("cycle_000/attempt_001")))
        worker.submit(PostprocessRequest(1, Path("cycle_001/attempt_001")))
        worker.close_and_drain()
        self.assertEqual(calls, [0, 1])

    def test_worker_product_failure_is_reported_without_losing_request_identity(self):
        worker = InlinePostprocessor(lambda request: (_ for _ in ()).throw(RuntimeError("plot failed")))
        worker.submit(PostprocessRequest(7, Path("cycle_007/attempt_001")))
        with self.assertRaisesRegex(RuntimeError, "cycle 7.*plot failed"):
            worker.check()
```

- [ ] **Step 2: Run and verify missing postprocessor failure**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_postprocess -v
```

Expected: import failure for `sil_repeat.postprocess`.

- [ ] **Step 3: Implement one spawn-safe worker process and injectable inline implementation**

```python
@dataclass(frozen=True)
class PostprocessRequest:
    cycle_index: int
    attempt_path: Path
```

Implement `ProcessPostprocessor.submit(request: PostprocessRequest) -> None`, `check() -> None`, and `close_and_drain() -> None`. Use `multiprocessing.get_context("spawn")`, one `JoinableQueue` for requests, and one result queue for success/failure records. The child process owns Matplotlib and the latest-result window. A plotting/display failure records the affected cycle and leaves raw acquisition intact; it must not issue hardware commands. `close_and_drain` sends one sentinel, waits for all queued work, joins with a bounded timeout, terminates only a stuck postprocessor child, and reports missing cycle products.

- [ ] **Step 4: Add a real-process smoke test with two tiny completed fixtures**

Use a temporary directory and two two-point cycle fixtures. Assert the subprocess exits with code 0, both `summary.json` files exist, and `repeat_summary.csv` contains two rows. Keep this test under 15 seconds in the VISA environment.

- [ ] **Step 5: Run focused tests and commit**

Run the Step 2 command, then:

```powershell
git add -- Code/Experiments/sil_repeat/postprocess.py Code/Debugs/test_sil_repeat_postprocess.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: postprocess repeated cycles asynchronously"
```

### Task 5: Fail-safe repeated acquisition runner

**Files:**
- Create: `Code/Experiments/sil_repeat/experiment.py`
- Create: `Code/Debugs/test_sil_repeat_experiment.py`

**Interfaces:**
- Consumes: `RepeatRunConfig`, `RepeatRunStore`, injected instrument bundle, `PostprocessRequest`, and postprocessor interface.
- Produces: `RepeatExperimentRunner.run() -> RepeatRunSummary`, `RepeatCycleSummary`, and `safe_shutdown` behavior identical to the established mandatory order.

- [ ] **Step 1: Write failing runner lifecycle tests with complete instrument fakes**

```python
class RepeatExperimentTests(unittest.TestCase):
    def test_runner_holds_gain_outputs_across_cycles_and_acquires_exact_schedule(self):
        config = tiny_config(cycles=2, points_per_branch=2)
        events, bundle = complete_fake_bundle()
        summary = RepeatExperimentRunner(config, store_for(config), lambda: bundle, postprocessor=recording_postprocessor(), sleep=lambda _: None).run()
        self.assertEqual(summary.acquisitions, 8)
        self.assertEqual(events.count("gain.enable_current"), 1)
        self.assertEqual(events.count("gain.disable_current"), 2)  # normalized baseline plus final shutdown
        self.assertNotIn("gain.disable_current", events[events.index("osa.acquire.1") + 1:events.index("osa.acquire.8")])

    def test_terminal_acquisition_failure_stops_later_voltage_commands_and_runs_full_cleanup(self):
        config = tiny_config(cycles=2, points_per_branch=2)
        events, bundle = failing_fake_bundle(osa_failure_at=3)
        with self.assertRaisesRegex(RuntimeError, "OSA failed"):
            RepeatExperimentRunner(config, store_for(config), lambda: bundle, postprocessor=recording_postprocessor(), sleep=lambda _: None).run()
        self.assertEqual(events[-6:], [
            "voltage.zero.emergency",
            "gain.disable_current",
            "gain.disable_tec",
            "gain.close",
            "voltage.close",
            "osa.close",
        ])
        self.assertEqual(events.count("osa.acquire"), 3)
```

- [ ] **Step 2: Run and verify the runner API failure**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_experiment -v
```

Expected: import failure for the new runner.

- [ ] **Step 3: Implement one interlocked setup followed by cycle-scoped acquisition**

The runner must execute this order exactly:

```python
self.osa.connect()
self.voltage.connect()
self.gain.connect()
self.gain.read_status()
self.voltage.zero(emergency=True)
self.gain.disable_current()
self.gain.set_temperature(config.operation_point.temperature_c)
if not self.gain.read_status().tec_enabled:
    self.gain.enable_tec()
self.gain.wait_stable(config.temperature_timeout_s)
self.gain.disable_current()
self.gain.enable_current()
self.gain.ramp_current(config.operation_point.current_ma, step_ma=config.current_ramp_step_ma, interval_s=config.current_ramp_interval_s)
self.sleep(config.ld_settle_s)
self.voltage.zero(emergency=True)
self.coupling_gate(config.operation_point)
```

For each pending cycle, create a new cycle attempt, command all 80 scheduled targets using `set_channel`, confirm each using `wait_for_channel`, sleep `settle_time_s`, acquire OSA trace A, read Gain telemetry, atomically save, and update rolling ETA. Mark the cycle `acquired` before submitting it for postprocessing. Do not zero voltage, disable current, or reconnect between cycles.

- [ ] **Step 4: Implement failure/resume and postprocessor shutdown ordering**

On a primary acquisition error, mark only the active cycle attempt failed, stop scheduling, and immediately execute mandatory hardware shutdown before waiting on postprocessing. On normal acquisition completion, zero voltage and disable current before draining the postprocessor; the outer `finally` still attempts all six cleanup actions. Resume skips only revalidated acquired/complete cycles and restarts a partial cycle in a new attempt.

- [ ] **Step 5: Add injected interrupt, cleanup-failure, and resume tests**

Cover `KeyboardInterrupt` during OSA acquisition, one cleanup action throwing while later actions still run, completed cycles skipped on resume, partial cycle restarted, and missing postprocessing regenerated without constructing instruments when all acquisitions are already complete.

- [ ] **Step 6: Run focused runner/storage tests and commit**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_experiment Code.Debugs.test_sil_repeat_storage Code.Debugs.test_sil_repeat_postprocess -v
git add -- Code/Experiments/sil_repeat/experiment.py Code/Debugs/test_sil_repeat_experiment.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: orchestrate repeated SIL acquisition"
```

### Task 6: CLI gates, simulation, approved config, and exact plan

**Files:**
- Create: `Code/Experiments/sil_repeat/run.py`
- Create: `Code/Experiments/sil_repeat/simulate.py`
- Create: `Code/Experiments/sil_repeat/repeat_24C_100mA.json`
- Create: `Code/Debugs/test_sil_repeat_cli.py`

**Interfaces:**
- Consumes: repeat config/store/runner, public `AQ6370`, `VoltageSource`, `GainDriver`, and deterministic simulation fakes.
- Produces: `python -m Code.Experiments.sil_repeat.run` with `--config`, `--resume`, `--plan`, `--simulate`, `--wait-for-coupling`, `--no-display`, and `--log-level`.

- [ ] **Step 1: Write failing plan and authorization tests**

```python
class RepeatCliTests(unittest.TestCase):
    def test_plan_reports_approved_run_without_constructing_factories(self):
        output = io.StringIO()
        calls = []
        code = main(["--config", str(APPROVED_CONFIG), "--plan"], output=output, factories=lambda: calls.append("constructed"))
        text = output.getvalue()
        self.assertEqual(code, 0)
        self.assertEqual(calls, [])
        for expected in ("24.0 degC", "100.0 mA", "V² range=40.0-90.0", "40", "100", "8000", "Result\\sil_repeat"):
            self.assertIn(expected, text)

    def test_real_run_requires_exact_RUN_before_driver_construction(self):
        calls = []
        code = main(["--config", str(APPROVED_CONFIG)], input_fn=lambda _: "run", factories=recording_factories(calls), output=io.StringIO())
        self.assertEqual(code, 0)
        self.assertEqual(calls, [])
```

- [ ] **Step 2: Run and verify missing CLI failure**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_cli -v
```

Expected: import failure for `sil_repeat.run`.

- [ ] **Step 3: Implement lazy real-driver construction and exact coupling gate**

Mirror the proven `sil_hysteresis.run` pattern: import real drivers only after exact `RUN`, print the validated plan before construction, hold at zero after interlocked current ramp, and require exact `COUPLED`. `--resume` must reject a mode/config mismatch before construction. The approved JSON must contain one operation point at 24.0 °C/100.0 mA, V² 40–90, 40 points per branch, 100 cycles, 0.1 s settle, channel 1, and output root `Result/sil_repeat`.

- [ ] **Step 4: Implement deterministic simulation without real drivers**

Reuse the complete public behavior of the existing simulation bundle, but make spectral branch state depend on the current cycle/direction schedule rather than inferring one global triangle. Supply a two-cycle test configuration for fast tests; do not simulate all 8,000 spectra in routine unit tests.

- [ ] **Step 5: Add exact 8,000-step dry-plan and two-cycle end-to-end tests**

Assert the dry plan constructs no drivers or output directory. Run a two-cycle similar simulation and verify 8 spectra for two points per branch, two complete cycle directories, six figure files, two summary rows, no `REAL` provenance, and mandatory simulated cleanup events.

- [ ] **Step 6: Run CLI plus focused end-to-end tests and commit**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest Code.Debugs.test_sil_repeat_cli Code.Debugs.test_sil_repeat_experiment Code.Debugs.test_sil_repeat_postprocess -v
git add -- Code/Experiments/sil_repeat/run.py Code/Experiments/sil_repeat/simulate.py Code/Experiments/sil_repeat/repeat_24C_100mA.json Code/Debugs/test_sil_repeat_cli.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "feat: add repeated SIL experiment CLI"
```

### Task 7: Operator documentation and final verification

**Files:**
- Create: `Code/Experiments/sil_repeat/README.md`
- Modify only if implementation evidence requires correction: `docs/superpowers/specs/2026-08-21-sil-repeat-scan-design.md`
- Test: all `Code/Debugs/test_sil_repeat_*.py`

**Interfaces:**
- Consumes: the final CLI and output schema.
- Produces: bench-ready commands, timing/capacity warning, interpretation notes, safe stop/resume instructions, and verified plan evidence.

- [ ] **Step 1: Write operator documentation from actual CLI output**

Document these exact commands:

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m Code.Experiments.sil_repeat.run --config .\Code\Experiments\sil_repeat\repeat_24C_100mA.json --plan
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m Code.Experiments.sil_repeat.run --config .\Code\Experiments\sil_repeat\repeat_24C_100mA.json --wait-for-coupling
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m Code.Experiments.sil_repeat.run --resume .\Result\sil_repeat\run_<id> --wait-for-coupling
```

Explain exact `RUN`/`COUPLED` gates, approximately 3.5-hour duration, several-hundred-megabyte capacity, 39 primary comparable levels despite 40 stored observations per branch, latest-window behavior, `Ctrl+C` shutdown, and partial-cycle restart.

- [ ] **Step 2: Run every targeted repeat test in the VISA environment**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest discover -s Code\Debugs -p 'test_sil_repeat_*.py' -v
```

Expected: all targeted tests pass with no warning/error output and no real driver construction.

- [ ] **Step 3: Run the approved dry plan and inspect every safety-critical line**

Run the first README command. Confirm exactly 24.0 °C, 100.0 mA, channel 1, V² 40–90, approximately 6.3246–9.4868 V, 40 observations per branch, 100 cycles, 8,000 total acquisitions, 0.1 s settle, coupling gate behavior, `Result\sil_repeat`, timing/capacity warning, and shutdown order.

- [ ] **Step 4: Run a fresh two-cycle simulation and verify artifacts**

Use a temporary two-cycle JSON under the test temporary directory, `--simulate similar --no-display`, then verify two complete status files, eight NPZ files for two points per branch, two PNG/PDF/SVG cycle sets, one aggregate PNG/PDF/SVG set, and exactly two aggregate CSV rows. Delete only the explicitly resolved temporary simulation directory after verifying it is outside `Code/Utils` and inside the test temporary root.

- [ ] **Step 5: Request code review and address only verified findings**

Use `superpowers:requesting-code-review` against the spec, this plan, the complete diff, and the fresh verification output. Any correction follows `superpowers:receiving-code-review` and TDD: reproduce a behavioral defect with a failing test before changing production code.

- [ ] **Step 6: Re-run verification and commit documentation/final corrections**

```powershell
& 'D:\SoftwareInstaller\Anaconda\envs\VISA\python.exe' -m unittest discover -s Code\Debugs -p 'test_sil_repeat_*.py' -v
git add -- Code/Experiments/sil_repeat/README.md Code/Experiments/sil_repeat Code/Debugs/test_sil_repeat_*.py
git -c user.name=Codex -c user.email=codex@localhost commit -m "docs: document repeated SIL bench run"
```

Before committing, inspect `git status --short` and keep the two pre-existing `sil_hysteresis` configuration changes out of the staged set.

### Task 8: Real-hardware handoff gate

**Files:**
- No code changes expected.
- Read: `Code/Experiments/sil_repeat/README.md`
- Read: `Code/Experiments/sil_repeat/repeat_24C_100mA.json`

**Interfaces:**
- Consumes: reviewed implementation, passing targeted tests, approved dry plan, and operator authorization.
- Produces: either a safe cancellation before construction or a live process paused at the `COUPLED` gate with 24.0 °C/100.0 mA active and Voltage Source at 0 V.

- [ ] **Step 1: Present the exact real plan and obtain fresh operator authorization**

Show the user the 3.5-hour estimate, 8,000 spectra, V² 40–90, 100 cycles, expected storage, shutdown behavior, and exact result root. Do not infer authorization from design or code approval.

- [ ] **Step 2: Start the real CLI only after exact `RUN`**

Run the second README command in a PTY. If the user does not provide exact uppercase `RUN`, allow the CLI to cancel without constructing drivers.

- [ ] **Step 3: Confirm safe coupling state before accepting `COUPLED`**

Report connected identities/ports, stable 24.0 °C TEC, verified 100.0 mA output, and all Voltage Source channels at zero. Accept exact uppercase `COUPLED` only after the operator confirms coupling.

- [ ] **Step 4: Monitor the live run and preserve safety priority**

Report cycle progress and rolling ETA without issuing unsolicited hardware changes. A user stop request or terminal fault ends acquisition and uses mandatory shutdown. After completion, verify 100 complete cycles, 8,000 NPZ files, 100 cycle figure sets, aggregate products, process exit code 0, and logged shutdown completion before claiming success.
