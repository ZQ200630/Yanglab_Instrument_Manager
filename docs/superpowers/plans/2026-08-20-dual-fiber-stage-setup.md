# Dual Fiber Stage Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a setup-specific, relative-displacement API for the registered left and right MAX312D fiber stages, with exact serial binding, logical laboratory coordinates, explicit session baselines, directional calibration, collision-conscious ordering, and no experiment-level MDT693B access.

**Architecture:** `Code.Setups.fiber_coupling` owns and wraps one or two existing `MDT693B` drivers. It discovers sides by exact serial metadata, maps logical X/Y/Z into side-specific controller axes, maintains only a session-local estimated position and confirmed command baseline, and delegates voltage ramps and authority enforcement to the generic driver. Configuration is strict JSON data; diagnostics and experiments use the setup layer rather than raw serial or the MDT driver.

**Tech Stack:** Python 3.10 in Anaconda environment `VISA`, standard-library `dataclasses`, `enum`, `json`, `pathlib`, `threading`, `argparse`, `unittest`, PySerial port enumeration, and the existing `Code.Utils.mdt693b` public API.

**Spec:** `docs/superpowers/specs/2026-08-20-dual-fiber-stage-setup-design.md`

## Global Constraints

- Run every Python command through `conda run --no-capture-output -n VISA python <module-or-script>`.
- Do not access real serial ports or hardware while implementing this plan. All discovery and MDT behavior are injected fakes.
- Do not modify `Code/Utils/mdt693b.py` to add setup identity, geometry, or calibration.
- Use only public MDT APIs: `connect()`, `close()`, `status`, `application_limits_v`, `adopt_current_axis_baseline(confirm=True)`, `get_all_voltages()`, and `set_axis_voltage(axis, target_v)`.
- If implementation appears to require an MDT private field or method, stop and report the missing public contract instead of bypassing it.
- Bind left/right only by exact serial numbers `2110148249-10` and `160721175410`; never bind by COM ordering.
- Logical coordinates are `+X` right, `+Y` away from the operator, and `+Z` up.
- Left mapping is logical X -> MDT Y, logical Y -> MDT X, logical Z -> MDT Z. Right mapping is identity. All configured polarities are positive.
- The left toward-chip direction is `+X`; the right toward-chip direction is `-X`.
- Hard per-call limits are exactly 0.2 um toward the chip and 1.0 um for every other per-axis direction. Configuration may reduce but never raise them.
- Version one is relative-only; do not add `move_to_um()` or persist estimated positions.
- Nominal MAX312D calibration is `0.2666666666666667 um/V` and requires explicit session authorization through baseline adoption.
- A motion failure, partial completion, cancellation, unconfirmed readback, authority loss, or unexplained actual-voltage change invalidates the whole side baseline. Never auto-zero or auto-rollback.
- All-axis preflight occurs before the first setter. Away-X moves execute X/Y/Z; toward-X moves execute Y/Z/X; zero-X moves execute Y/Z.
- Preserve user changes and unrelated files. The workspace is not currently a Git repository; do not initialize Git without user authorization.
- At every task checkpoint, record the focused command and result in the execution report. If this plan is executed later in a Git-backed copy, use the suggested commit message; otherwise skip the commit without changing repository state.

## File Responsibility Map

- Create `Code/Setups/__init__.py`: export only the setup-layer public types, errors, and `FiberCouplingSetup`.
- Create `Code/Setups/fiber_coupling.py`: immutable types, strict configuration loader, discovery, side handles, baseline lifecycle, motion planning/execution, cleanup, and exception evidence.
- Create `Config/fiber_coupling.json`: approved serial bindings, axis maps, polarity, MAX312D model, directional calibration, and operator limits.
- Create `Code/Debugs/test_fiber_coupling.py`: all setup-layer offline tests with fake ports and fake MDT drivers.
- Create `Code/Debugs/check_fiber_stages.py`: staged enumeration/read-only/move diagnostic using only the setup API.
- Modify `AGENTS.md`: persist the setup boundary, coordinate mapping, limits, estimate semantics, and diagnostic rules.
- Do not export setup types from `Code/Utils/__init__.py`; consumers import them from `Code.Setups`.

---

### Task 1: Immutable Types and Strict Configuration

**Files:**
- Create: `Code/Setups/__init__.py`
- Create: `Code/Setups/fiber_coupling.py`
- Create: `Config/fiber_coupling.json`
- Create: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: `DriverError`, `DeviceFault`, `InstrumentConnectionError`, and `InstrumentSafetyError` from `Code.Utils.common`; `Axis`, `AxisState`, `MDT693B`, and `MDTStatus` from `Code.Utils.mdt693b`.
- Produces: `StageSide`, `LogicalAxis`, `Vector3Um`, `CalibrationCoefficient`, `AxisCalibration`, `StageDefinition`, `FiberCouplingConfig`, `SerialDeviceInfo`, `FiberDiscovery`, `StageStatus`, `MoveResult`, the six approved setup errors, and `load_fiber_coupling_config(path)`.

- [ ] **Step 1: Write configuration and immutable-type tests first**

Add `FiberConfigTests` to `Code/Debugs/test_fiber_coupling.py`. The first test must import the not-yet-created module so the initial run proves the feature is absent. Cover the exact default mapping and all strict rejections.

```python
class FiberConfigTests(unittest.TestCase):
    def test_default_config_has_exact_registered_topology(self):
        config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        left = config.stages[StageSide.LEFT]
        right = config.stages[StageSide.RIGHT]

        self.assertEqual(left.serial_number, "2110148249-10")
        self.assertEqual(right.serial_number, "160721175410")
        self.assertEqual(left.axis_map[LogicalAxis.X], Axis.Y)
        self.assertEqual(left.axis_map[LogicalAxis.Y], Axis.X)
        self.assertEqual(left.axis_map[LogicalAxis.Z], Axis.Z)
        self.assertEqual(right.axis_map[LogicalAxis.X], Axis.X)
        self.assertEqual(right.axis_map[LogicalAxis.Y], Axis.Y)
        self.assertEqual(right.axis_map[LogicalAxis.Z], Axis.Z)
        self.assertEqual(left.toward_chip_sign, 1)
        self.assertEqual(right.toward_chip_sign, -1)

    def test_default_config_has_twelve_nominal_directional_coefficients(self):
        config = load_fiber_coupling_config(DEFAULT_CONFIG_PATH)
        coefficients = [
            coefficient
            for stage in config.stages.values()
            for axis in stage.calibration.values()
            for coefficient in (axis.positive, axis.negative)
        ]
        self.assertEqual(len(coefficients), 12)
        for coefficient in coefficients:
            self.assertEqual(coefficient.um_per_v, 0.2666666666666667)
            self.assertEqual(coefficient.source, "nominal_MAX312D")

    def test_vector_rejects_bool_nan_infinity_and_non_numbers(self):
        for bad in (True, float("nan"), float("inf"), "0.1", None):
            with self.subTest(bad=bad):
                with self.assertRaises(InstrumentSafetyError):
                    Vector3Um(bad, 0.0, 0.0)
```

Use temporary JSON files to test: missing side; extra side; duplicate serial; incomplete/duplicate axis permutation; polarity other than exact integer `-1` or `+1`; model other than `MAX312D`; missing positive/negative calibration; bool/string/NaN/infinity/nonpositive coefficient; unrecognized source; operator toward limit over 0.2; other limit over 1.0; negative limits; and unexpected keys. Prove every loaded mapping is immutable.

- [ ] **Step 2: Run focused tests and capture RED**

Run:

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberConfigTests -v
```

Expected: import error for missing `Code.Setups.fiber_coupling` or missing public names. No production file beyond the test exists before this RED.

- [ ] **Step 3: Create the exact default JSON configuration**

Create `Config/fiber_coupling.json` with this schema and repeat the calibration object for all six side/axis entries:

```json
{
  "model": "MAX312D",
  "operator_limits_um": {
    "toward_chip": 0.2,
    "other": 1.0
  },
  "stages": {
    "left": {
      "serial_number": "2110148249-10",
      "toward_chip_sign": 1,
      "axis_map": {"x": "Y", "y": "X", "z": "Z"},
      "polarity": {"x": 1, "y": 1, "z": 1},
      "calibration": {
        "x": {
          "positive": {
            "um_per_v": 0.2666666666666667,
            "source": "nominal_MAX312D",
            "date": null,
            "note": "MAX312D nominal 20 um over 75 V"
          },
          "negative": {
            "um_per_v": 0.2666666666666667,
            "source": "nominal_MAX312D",
            "date": null,
            "note": "MAX312D nominal 20 um over 75 V"
          }
        }
      }
    },
    "right": {
      "serial_number": "160721175410",
      "toward_chip_sign": -1,
      "axis_map": {"x": "X", "y": "Y", "z": "Z"},
      "polarity": {"x": 1, "y": 1, "z": 1},
      "calibration": {}
    }
  }
}
```

The actual file must contain complete `x`, `y`, and `z` calibration records for both sides; empty or omitted records are test fixtures only and must fail loading.

- [ ] **Step 4: Implement public enums, dataclasses, errors, and defensive immutability**

Use these exact public signatures in `Code/Setups/fiber_coupling.py`:

```python
class StageSide(Enum):
    LEFT = "left"
    RIGHT = "right"

class LogicalAxis(Enum):
    X = "x"
    Y = "y"
    Z = "z"

@dataclass(frozen=True)
class Vector3Um:
    x: float
    y: float
    z: float

    def __post_init__(self) -> None:
        object.__setattr__(self, "x", _finite_motion_float(self.x, "x"))
        object.__setattr__(self, "y", _finite_motion_float(self.y, "y"))
        object.__setattr__(self, "z", _finite_motion_float(self.z, "z"))

    def for_axis(self, axis: LogicalAxis) -> float:
        return {LogicalAxis.X: self.x, LogicalAxis.Y: self.y, LogicalAxis.Z: self.z}[axis]

@dataclass(frozen=True)
class CalibrationCoefficient:
    um_per_v: float
    source: str
    date: str | None
    note: str

@dataclass(frozen=True)
class AxisCalibration:
    positive: CalibrationCoefficient
    negative: CalibrationCoefficient

@dataclass(frozen=True)
class StageDefinition:
    side: StageSide
    serial_number: str
    toward_chip_sign: int
    axis_map: Mapping[LogicalAxis, Axis]
    polarity: Mapping[LogicalAxis, int]
    calibration: Mapping[LogicalAxis, AxisCalibration]

@dataclass(frozen=True)
class FiberCouplingConfig:
    model: str
    stages: Mapping[StageSide, StageDefinition]
    toward_chip_limit_um: float
    other_limit_um: float

@dataclass(frozen=True)
class SerialDeviceInfo:
    resource: str
    serial_number: str
    description: str

@dataclass(frozen=True)
class FiberDiscovery:
    registered: Mapping[StageSide, SerialDeviceInfo]
    missing_sides: frozenset[StageSide]
    unknown_devices: tuple[SerialDeviceInfo, ...]
```

Copy every caller-provided mapping and wrap it in `MappingProxyType` during `__post_init__`. Define `StageStatus` and `MoveResult` now with the exact fields later tasks publish:

```python
@dataclass(frozen=True)
class StageStatus:
    side: StageSide
    available: bool
    serial_number: str
    resource: str | None
    toward_chip_sign: int
    toward_chip_limit_um: float
    other_limit_um: float
    baseline_known: bool
    estimated_position_um: Vector3Um | None
    observed_voltage_v: Mapping[LogicalAxis, float] | None
    calibration: Mapping[LogicalAxis, AxisCalibration]
    nominal_authorized: bool
    restricted: bool
    fault: str | None

@dataclass(frozen=True)
class MoveResult:
    side: StageSide
    requested_um: Vector3Um
    voltage_delta_v: Mapping[LogicalAxis, float]
    calibration_used: Mapping[LogicalAxis, CalibrationCoefficient]
    estimated_before_um: Vector3Um
    estimated_after_um: Vector3Um
    observed_voltage_v: Mapping[LogicalAxis, float]
    confirmed: bool
```

Define the approved error names using the existing taxonomy:

```python
class StageUnavailableError(InstrumentConnectionError):
    """The requested registered setup side is not connected."""

class BaselineUnknownError(InstrumentSafetyError):
    """Relative motion lacks a trustworthy session command baseline."""

class CalibrationRequiredError(InstrumentSafetyError):
    """The active nominal calibration lacks explicit authorization."""

class StageMotionLimitError(InstrumentSafetyError):
    """A requested displacement or derived voltage exceeds a limit."""

class StageConnectionError(InstrumentConnectionError):
    """Setup discovery, identity verification, or cleanup failed."""

class StageMotionError(DeviceFault):
    """An attempted stage motion could not be confirmed safely."""
```

- [ ] **Step 5: Implement a fail-closed JSON loader**

Implement:

```python
DEFAULT_CONFIG_PATH = Path(__file__).resolve().parents[2] / "Config" / "fiber_coupling.json"
_HARD_TOWARD_CHIP_LIMIT_UM = 0.2
_HARD_OTHER_LIMIT_UM = 1.0

def load_fiber_coupling_config(
    path: str | Path = DEFAULT_CONFIG_PATH,
) -> FiberCouplingConfig:
    raw = _read_strict_json(Path(path))
    return _parse_fiber_coupling_config(raw)
```

Implement `_read_strict_json(path) -> object` and `_parse_fiber_coupling_config(raw) -> FiberCouplingConfig` in the same module. Read UTF-8 JSON, require exact object keys at every safety-bearing level, reject JSON constants `NaN`/`Infinity` via `parse_constant`, reject bool where a number is required, and validate all constraints listed in Step 1 before constructing immutable objects. Compare operator limits against the two code constants so JSON can lower but never raise them. Accept calibration sources only when equal to `nominal_MAX312D` or `measured`. Validate date as `None` or nonempty text; validate note as text. Raise `InstrumentSafetyError` with the JSON field path in every message and retain JSON/I/O errors as causes.

- [ ] **Step 6: Export only the setup public surface**

Create `Code/Setups/__init__.py` importing and listing in `__all__`:

```python
FiberCouplingSetup, FiberStage, StageSide, LogicalAxis, Vector3Um,
CalibrationCoefficient, AxisCalibration, SerialDeviceInfo, FiberDiscovery,
StageStatus, MoveResult,
StageUnavailableError, BaselineUnknownError, CalibrationRequiredError,
StageMotionLimitError, StageConnectionError, StageMotionError,
load_fiber_coupling_config
```

`FiberCouplingSetup` and `FiberStage` may be minimal nonfunctional declarations until Task 2, but do not create methods that return fake success.

- [ ] **Step 7: Run focused GREEN and compile checks**

Run:

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberConfigTests -v
conda run --no-capture-output -n VISA python -m py_compile Code/Setups/__init__.py Code/Setups/fiber_coupling.py Code/Debugs/test_fiber_coupling.py
```

Expected: all Task 1 tests pass; compilation exits 0.

- [ ] **Step 8: Record Task 1 checkpoint and review**

Verify the default file contains twelve complete coefficients, no mutable mapping escapes, and no serial enumeration/hardware construction occurs at import or load time. Suggested Git commit if a Git repository exists: `feat: add fiber stage setup configuration`.

---

### Task 2: Serial Discovery, Side Availability, and Resource Ownership

**Files:**
- Modify: `Code/Setups/fiber_coupling.py`
- Modify: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: `FiberCouplingConfig`, `StageDefinition`, `StageSide`, `StageStatus`, `MDT693B(port)`, `MDT693B.connect()`, `MDT693B.status`, and `MDT693B.close()`.
- Produces: `FiberCouplingSetup.enumerate(config, *, config_path, port_enumerator) -> FiberDiscovery`, `FiberCouplingSetup.connect(config, *, config_path, port_enumerator, driver_factory)`, stable `.left`/`.right` handles, `available_sides`, `missing_sides`, `unknown_devices`, context-manager support, and truthful `close()`.

- [ ] **Step 1: Add injected fake port and fake MDT fixtures**

Add deterministic fakes to the test file rather than importing private MDT test helpers:

```python
@dataclass(frozen=True)
class FakePort:
    device: str
    serial_number: str | None
    description: str = "Thorlabs MDT693B"
    manufacturer: str = "Thorlabs"
    product: str = "MDT693B"

class FakeSetupMDT:
    def __init__(self, port, *, status, connect_error=None, close_error=None):
        self.port = port
        self.status = status
        self.application_limits_v = MappingProxyType({axis: 75.0 for axis in Axis})
        self.connect_error = connect_error
        self.close_error = close_error
        self.connect_calls = 0
        self.close_calls = 0
        self.set_calls = []

    def connect(self):
        self.connect_calls += 1
        if self.connect_error is not None:
            raise self.connect_error
        return self

    def close(self):
        self.close_calls += 1
        if self.close_error is not None:
            raise self.close_error
```

Provide `make_mdt_status(serial: str, *, axis_command_known: bool = False, restricted: bool = False, fault_evidence: str | None = None, actual_v: Mapping[Axis, float] | None = None) -> MDTStatus` using the real immutable `MDTStatus` type.

- [ ] **Step 2: Write discovery and cleanup tests**

Add `FiberDiscoveryTests` covering:

```python
def test_left_only_binds_left_by_exact_serial_and_right_is_unavailable(self):
    setup = FiberCouplingSetup.connect(
        config=self.config,
        port_enumerator=lambda: [FakePort("COM6", "2110148249-10")],
        driver_factory=self.factory,
    )
    self.assertTrue(setup.left.status.available)
    self.assertFalse(setup.right.status.available)
    self.assertEqual(setup.available_sides, frozenset({StageSide.LEFT}))
    with self.assertRaises(StageUnavailableError):
        setup.right.adopt_baseline(confirm=True, allow_nominal=True)
    self.assertEqual(self.factory.instances["COM6"].set_calls, [])
```

Also cover right-only, both present, neither present, unknown alongside known, unknown-only, duplicate known serial metadata, exact identity mismatch after connect, known-side connection failure after another side opened, context-body plus two close failures, idempotent close, and no false disconnected state when a fake retains an open resource. Assert unknown devices are never passed to the driver factory.

- [ ] **Step 3: Run focused RED**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberDiscoveryTests -v
```

Expected: failures for missing connection/discovery behavior; config tests remain green.

- [ ] **Step 4: Implement discovery metadata classification**

Add injected class methods with these exact interfaces:

- `FiberCouplingSetup.enumerate(config: FiberCouplingConfig | None = None, *, config_path: str | Path = DEFAULT_CONFIG_PATH, port_enumerator: Callable[[], Iterable[object]] = list_ports.comports) -> FiberDiscovery`
- `FiberCouplingSetup.connect(config: FiberCouplingConfig | None = None, *, config_path: str | Path = DEFAULT_CONFIG_PATH, port_enumerator: Callable[[], Iterable[object]] = list_ports.comports, driver_factory: Callable[..., MDT693B] = MDT693B) -> FiberCouplingSetup`

Enumerate once. Read only metadata attributes `device`, `serial_number`, `description`, `manufacturer`, and `product`. A port is an unknown MDT candidate only if its descriptive metadata identifies Thorlabs/MDT693B and its nonempty serial is not registered. Never open it.

Build an exact registered-serial index. Reject duplicate occurrences of a registered serial before constructing any driver. If no registered serial is present, `connect()` raises `StageConnectionError` after enumeration and creates zero drivers.

`enumerate()` itself does not require at least one registered device; it returns a truthful `FiberDiscovery` even when both sides are missing. `connect()` applies the at-least-one rule. Copy and freeze the registered mapping and sort unknown devices deterministically by `(serial_number, resource)`.

- [ ] **Step 5: Connect known devices and verify read-only identity**

For each discovered registered side, construct `driver_factory(port.device)`, call `connect()`, require non-`None` public status, and compare `status.serial_number` exactly to the registered serial. Do not adopt a baseline or call a setter.

If any discovered registered side fails construction, connection, identity verification, or status publication, close every driver already created. Raise the earliest typed `StageConnectionError` and attach cleanup failures without replacing it. A registered side absent from enumeration remains an intentionally unavailable handle.

- [ ] **Step 6: Implement stable handles and truthful close**

Create two `FiberStage` handles during setup construction. An unavailable handle retains its `StageDefinition` but has no driver/resource. Its status maps to `available=False`, `baseline_known=False`, `estimated_position_um=None`, and zero fault-free I/O.

`FiberCouplingSetup.close()` must:

1. publish setup/side close intent before waiting for operations;
2. call close on every owned available driver, even if an earlier close fails;
3. retain the earliest error as primary and attach later errors;
4. remain retryable when a driver still reports an open/faulted ownership condition;
5. be idempotent after confirmed complete cleanup.

Implement `__enter__` and `__exit__` so a body exception remains primary and close errors are attached.

- [ ] **Step 7: Run focused GREEN and Task 1 regression**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberDiscoveryTests Code.Debugs.test_fiber_coupling.FiberConfigTests -v
```

Expected: all pass; fake unknown ports have zero driver constructions and all opened fakes are closed by the tests.

- [ ] **Step 8: Record Task 2 checkpoint and review**

Audit that COM order never influences side selection, a missing side is distinct from a failed present side, and no output-changing API appears in connect/close paths. Suggested Git commit: `feat: discover fiber stages by serial identity`.

---

### Task 3: Baseline Adoption, Logical Status, and Authority Invalidation

**Files:**
- Modify: `Code/Setups/fiber_coupling.py`
- Modify: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: available `FiberStage`, `MDT693B.adopt_current_axis_baseline(confirm=True) -> MDTStatus`, current public `MDT693B.status`, and `MDTStatus.axis_command_known`.
- Produces: `FiberStage.adopt_baseline(*, confirm=False, allow_nominal=False) -> StageStatus`, logical `FiberStage.status`, setup-owned command baseline, and session-only estimated zero.

- [ ] **Step 1: Extend the fake MDT with baseline and observation behavior**

Add exact fake methods and call records:

```python
def adopt_current_axis_baseline(self, *, confirm=False):
    self.adopt_calls.append(confirm)
    if self.adopt_error is not None:
        raise self.adopt_error
    self.status = replace(self.status, axis_command_known=True)
    return self.status

def get_all_voltages(self):
    self.get_all_calls += 1
    return MappingProxyType({axis: state.actual_v for axis, state in self.status.axes.items()})
```

The fake must support replacing status with `axis_command_known=False`, changed actual values, restrictions, and fault evidence without the setup accessing fake-only fields.

- [ ] **Step 2: Write baseline lifecycle tests**

Add `FiberBaselineTests` for unavailable side, missing `confirm=True`, nominal config without `allow_nominal=True`, a mixed nominal/measured config that still requires authorization, fully measured config without nominal authorization, exact adoption call, left/right logical voltage mapping, estimated zero, reconnect reset, close reset, authority loss, status-observed external change, restricted/fault publication, and immutable status.

Representative assertion:

```python
status = setup.left.adopt_baseline(confirm=True, allow_nominal=True)
self.assertEqual(driver.adopt_calls, [True])
self.assertEqual(status.estimated_position_um, Vector3Um(0.0, 0.0, 0.0))
self.assertEqual(
    status.observed_voltage_v,
    {
        LogicalAxis.X: driver.status.axes[Axis.Y].actual_v,
        LogicalAxis.Y: driver.status.axes[Axis.X].actual_v,
        LogicalAxis.Z: driver.status.axes[Axis.Z].actual_v,
    },
)
self.assertTrue(status.baseline_known)
self.assertTrue(status.nominal_authorized)
```

Add a probe that changes public MDT status to `axis_command_known=False`, reads `stage.status`, and proves `baseline_known=False`, `estimated_position_um is None`, and nominal authorization is cleared.

- [ ] **Step 3: Run baseline tests and capture RED**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberBaselineTests -v
```

Expected: missing adoption/status synchronization failures; no setter calls.

- [ ] **Step 4: Implement logical status mapping**

Add private helpers that map only public `MDTStatus.axes[controller_axis].actual_v` into logical axes according to `StageDefinition.axis_map`. Never access `_axis_commands_v`, `_last_confirmed_actual_v`, `_serial`, or `_protocol`.

On every `stage.status` read, synchronize authority under the side lock:

```python
if driver.status is None or not driver.status.axis_command_known:
    self._invalidate_baseline_unlocked("MDT axis-command authority is unknown")
```

When a baseline is known, also map the current public actual outputs and compare them with the setup-owned command baseline using `math.isclose(rel_tol=0.0, abs_tol=1e-6)`. Any unexplained mismatch invalidates the baseline even if a fake or future driver status still claims authority. A restricted or faulted public status likewise invalidates displacement authority.

Map `restricted` and `fault_evidence` truthfully. An unavailable status contains the registered serial but no resource or observed voltage.

- [ ] **Step 5: Implement explicit baseline adoption**

Use the exact public interface `FiberStage.adopt_baseline(*, confirm: bool = False, allow_nominal: bool = False) -> StageStatus`.

Require exact booleans. Reject absent side, close intent, missing confirmation, and nominal calibration without nominal permission before invoking MDT adoption. Call the public adoption once. Require returned `axis_command_known=True`, no driver fault, and no restricted status. Map the returned public actual voltages into a setup-owned logical command baseline, set estimated position to exact zero, and publish calibration/authorization atomically.

If adoption throws or returns unconfirmed authority, do not retain a baseline. Preserve the underlying exception as cause of the appropriate typed setup error.

- [ ] **Step 6: Run baseline GREEN and discovery/config regression**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberBaselineTests Code.Debugs.test_fiber_coupling.FiberDiscoveryTests Code.Debugs.test_fiber_coupling.FiberConfigTests -v
```

Expected: all pass and every baseline test has zero setter calls.

- [ ] **Step 7: Record Task 3 checkpoint and review**

Review state transitions for reconnect and close. Confirm estimates are never written to JSON and that nominal permission disappears with baseline authority. Suggested Git commit: `feat: add explicit fiber stage baselines`.

---

### Task 4: Pure Relative-Motion Planning and Complete Preflight

**Files:**
- Modify: `Code/Setups/fiber_coupling.py`
- Modify: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: a known setup command baseline, `Vector3Um`, directional calibration, public fresh `get_all_voltages()`, `MDTStatus` axis/hardware limits, and `driver.application_limits_v`.
- Produces: private immutable `_AxisMove` and `_MovePlan` values plus `_plan_move_unlocked(requested) -> _MovePlan`. It sends no setters.

- [ ] **Step 1: Write directional conversion and limit tests**

Add `FiberMotionPlanningTests`. Use deliberately different twelve coefficients so every side/axis/sign selection is observable. Cover exact formulas, left swap, right identity, all positive polarities, and immutable plans.

```python
with setup.left._operation_lock:
    plan = setup.left._plan_move_unlocked(Vector3Um(0.10, -0.20, 0.30))
self.assertEqual(plan.moves[0].logical_axis, LogicalAxis.Y)  # toward X is last
self.assertEqual(plan.moves[1].logical_axis, LogicalAxis.Z)
self.assertEqual(plan.moves[2].logical_axis, LogicalAxis.X)
self.assertEqual(plan.moves[2].controller_axis, Axis.Y)
self.assertEqual(
    plan.moves[2].delta_v,
    0.10 / self.left_x_positive.um_per_v,
)
```

Do not add a test-only production method. Task 4 tests may call the real private `_plan_move_unlocked` under its operation lock; Task 5 must repeat the critical ordering and boundary assertions through public `move_by_um()`.

- [ ] **Step 2: Write exact safety-boundary tests**

Cover:

- left X `+0.2` accepted and `+nextafter(0.2, +inf)` rejected;
- right X `-0.2` accepted and a more negative adjacent float rejected;
- left `-1.0`, right `+1.0`, every Y/Z sign at `1.0` accepted;
- just beyond every nonapproach limit rejected;
- configuration can reduce but not raise limits;
- target exactly 0 and 75 accepted when all other limits permit;
- target immediately below 0 or above 75 rejected;
- hardware, application, axis minimum, and axis maximum each independently bind;
- bool/NaN/infinity/non-number rejected;
- one bad axis in a three-axis request results in zero setter calls;
- zero vector still requires an available stage and known baseline, performs the fresh read-only preflight, and produces a no-write plan.

- [ ] **Step 3: Write fresh-authority and observation tests**

Before planning, the setup must call `get_all_voltages()` and then inspect the resulting public status. Test that a fake changes actual voltage or clears authority during this query. Planning must raise `BaselineUnknownError`, clear the estimate, and produce zero setters. A public status with `restricted=True` or `fault_evidence` cannot plan displacement motion.

- [ ] **Step 4: Run planning tests and capture RED**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberMotionPlanningTests -v
```

Expected: missing planner and boundary failures; fake setter lists remain empty.

- [ ] **Step 5: Implement private plan values and exact arithmetic**

```python
@dataclass(frozen=True)
class _AxisMove:
    logical_axis: LogicalAxis
    controller_axis: Axis
    requested_um: float
    coefficient: CalibrationCoefficient
    delta_v: float
    start_v: float
    target_v: float

@dataclass(frozen=True)
class _MovePlan:
    requested_um: Vector3Um
    moves: tuple[_AxisMove, ...]
    voltage_delta_v: Mapping[LogicalAxis, float]
    calibration_used: Mapping[LogicalAxis, CalibrationCoefficient]
```

Use exact command comparisons for requested limits and voltage bounds; do not add a motion-confirmation tolerance to caller targets. Select positive calibration for requested values greater than zero and negative calibration for values less than zero. Compute:

```python
delta_v = requested_um / coefficient.um_per_v / polarity
target_v = setup_command_baseline_v[logical_axis] + delta_v
```

Reject non-finite derived values. Check target against exact `[0, 75]`, `status.hardware_limit.volts`, `driver.application_limits_v[controller_axis]`, and `status.axes[controller_axis].minimum_v/maximum_v`.

- [ ] **Step 6: Implement collision-conscious ordering**

Classify logical X using `toward_chip_sign`:

```python
toward = requested.x * definition.toward_chip_sign > 0.0
away = requested.x * definition.toward_chip_sign < 0.0
order = (
    (LogicalAxis.Y, LogicalAxis.Z, LogicalAxis.X) if toward
    else (LogicalAxis.X, LogicalAxis.Y, LogicalAxis.Z) if away
    else (LogicalAxis.Y, LogicalAxis.Z)
)
```

Skip zero axes. Build the complete plan only after all axes validate, so an invalid later axis cannot leave a partial plan or trigger a setter.

- [ ] **Step 7: Run focused GREEN and verify zero setters**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberMotionPlanningTests -v
```

Expected: all pass; the fake's setter list remains empty in every planning-only test.

- [ ] **Step 8: Record Task 4 checkpoint and review**

Mutation-check the two approach signs, the left X/Y mapping, and the exact 0.2/1.0 comparisons; each mutation must fail a focused test. Restore production immediately after each mutation. Suggested Git commit: `feat: plan calibrated fiber stage motion`.

---

### Task 5: Confirmed Motion Execution, Failure Truth, and Concurrency

**Files:**
- Modify: `Code/Setups/fiber_coupling.py`
- Modify: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: `_MovePlan`, `MDT693B.set_axis_voltage(axis, target_v)`, `get_all_voltages()`, `MDTStatus.axis_command_known`, and setup close intent.
- Produces: `FiberStage.move_by_um(dx=0.0, dy=0.0, dz=0.0) -> MoveResult`, session estimate updates, failure invalidation, and per-side operation serialization.

- [ ] **Step 1: Extend the fake with gated setters and status evolution**

Implement the fake's setter using public-behavior semantics:

```python
def set_axis_voltage(self, axis, target_v):
    self.set_calls.append((axis, target_v))
    if self.setter_gate is not None:
        self.setter_entered.set()
        self.setter_gate.wait(1.0)
    error = self.set_errors.get(len(self.set_calls))
    if error is not None:
        raise error
    axes = dict(self.status.axes)
    axes[axis] = replace(axes[axis], actual_v=target_v)
    self.status = replace(self.status, axes=axes, axis_command_known=True)
    return self.status
```

Allow injected readback mismatch, authority loss, get-all error, close cancellation, and BaseException at each phase.

- [ ] **Step 2: Write successful move and result tests**

Add `FiberMotionExecutionTests` for one-axis and three-axis calls on each side. Assert exact setter axis/target/order, one final public all-voltage read, immutable evidence, calibration provenance, and position accumulation over repeated successful moves.

```python
result = setup.right.move_by_um(dx=-0.1, dy=0.2, dz=0.3)
self.assertEqual([axis for axis, _ in driver.set_calls], [Axis.X, Axis.Y, Axis.Z])
self.assertEqual(result.requested_um, Vector3Um(-0.1, 0.2, 0.3))
self.assertEqual(result.estimated_before_um, Vector3Um(0.0, 0.0, 0.0))
self.assertEqual(result.estimated_after_um, Vector3Um(-0.1, 0.2, 0.3))
self.assertTrue(result.confirmed)
```

For a toward move, assert Y/Z/X. For an away move, assert X/Y/Z. A zero vector performs no setter and returns unchanged estimated position only after availability, baseline, and fresh read-only validation succeed.

- [ ] **Step 3: Write partial failure and no-rollback tests**

Inject failure on the second setter of a three-axis move. Assert:

- the first setter occurred;
- the third setter did not occur;
- no compensating setter or zero command occurred;
- `StageMotionError` retains the exact original error as `__cause__`;
- stage status has `baseline_known=False`, `estimated_position_um=None`, and `nominal_authorized=False`;
- the other stage has zero extra calls and retains its baseline.

Repeat for setter timeout, final `get_all_voltages()` failure, final readback mismatch/authority loss, `KeyboardInterrupt`, and `SystemExit`. Preserve the earliest BaseException identity for non-`Exception` failures while still invalidating local state.

- [ ] **Step 4: Write close/motion and same-side concurrency tests**

Use events rather than sleeps. Gate the first setter, start a move thread, start close, prove close intent becomes visible, release the fake, and assert no later axis setter can begin. The underlying close error/result must remain truthful and the stage baseline must be unknown.

Queue two moves on the same side and prove they do not interleave. Run left and right fake moves concurrently and prove the setup imposes no shared motion lock. Do not assert real-time synchronization.

- [ ] **Step 5: Run motion tests and capture RED**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberMotionExecutionTests -v
```

Expected: missing public motion method and concurrency/failure semantics.

- [ ] **Step 6: Implement public motion with operation-wide truth handling**

Use the exact public interface `FiberStage.move_by_um(dx: float = 0.0, dy: float = 0.0, dz: float = 0.0) -> MoveResult`.

Algorithm under the side operation lock:

1. Check close intent before and after lock acquisition.
2. Construct `Vector3Um` and perform the fresh complete-vector plan.
3. Save immutable pre-move estimate and a working copy of the setup command baseline.
4. For each `_AxisMove`, recheck close intent and call `driver.set_axis_voltage(controller_axis, target_v)`.
5. After every successful setter, require returned/public status authority still known and update only the private working command copy.
6. After all setters, call `get_all_voltages()`, require public status authority, and compare each moved controller output with the target using the MDT public reporting tolerance of `1e-6 V` (`rel_tol=0.0`).
7. Only then atomically publish the new setup command baseline and `estimated_position + requested`.
8. Return an immutable `MoveResult` with logical observations and calibration evidence.

On any failure after planning begins, distinguish zero-setter local/preflight failures from attempted motion. Any attempted motion or uncertain phase invalidates the baseline before re-raising. Translate ordinary underlying driver errors to `StageMotionError` with `__cause__`; preserve `KeyboardInterrupt`/`SystemExit` identity and attach stage fault evidence without replacing them.

- [ ] **Step 7: Implement per-side locking and close intent**

Each `FiberStage` owns an operation `RLock` and a close-intent `Event`. Setup close sets intent on all handles before calling driver close so queued work rejects before I/O. Do not hold a setup-global lock across device I/O. This lets independent sides operate concurrently while serializing each individual side.

- [ ] **Step 8: Run focused and all setup tests GREEN**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberMotionExecutionTests -v
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling -v
```

Expected: all setup tests pass and no test thread remains alive.

- [ ] **Step 9: Record Task 5 checkpoint and review**

Audit every setter call site: it must be inside `move_by_um`, operate on a planned controller axis/target, and be unreachable without baseline/authority/limit checks. Mutation-remove the whole-vector preflight and the post-lock close gate to prove focused tests fail, then restore. Suggested Git commit: `feat: execute confirmed fiber stage moves`.

---

### Task 6: Staged Fiber-Stage Diagnostic CLI

**Files:**
- Create: `Code/Debugs/check_fiber_stages.py`
- Modify: `Code/Debugs/test_fiber_coupling.py`

**Interfaces:**
- Consumes: `FiberCouplingSetup.connect()`, setup/side status, `adopt_baseline()`, and `move_by_um()` only. It must not import `MDT693B`, `Axis`, or `serial.Serial`.
- Produces: `_parser()`, `main(argv=None) -> int`, `--enumerate`, `--read-only`, and explicit `--move --side --axis --um [--allow-nominal]` workflows.

- [ ] **Step 1: Write CLI parser and zero-write stage tests**

Add `FiberDiagnosticTests` with patched setup factories and captured stdout/stderr. Cover:

- `--help` exits 0;
- exactly one of `--enumerate`, `--read-only`, or `--move` is required;
- move requires `--side`, `--axis`, and `--um`;
- NaN/infinity/bool-like text and zero displacement are rejected by argparse;
- enumeration prints available/missing/unknown identities and performs no connect;
- read-only prints only logical X/Y/Z, baseline state, and calibration source;
- unavailable requested side fails before adoption;
- nominal move without `--allow-nominal` performs zero adoption/setter calls;
- EOF, interruption, or wrong confirmation text performs zero setters.

- [ ] **Step 2: Run diagnostic tests and capture RED**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberDiagnosticTests -v
```

Expected: import error for missing `check_fiber_stages` or missing parser/main.

- [ ] **Step 3: Implement mutually exclusive staged parser**

Use these public arguments:

```text
--enumerate
--read-only
--move --side {left,right} --axis {x,y,z} --um FLOAT [--allow-nominal]
--config PATH
```

`--enumerate` calls `FiberCouplingSetup.enumerate()` and prints its immutable `FiberDiscovery` without constructing MDT drivers. Do not duplicate serial matching rules inside the diagnostic.

`--read-only` connects through `FiberCouplingSetup`, prints all available sides in logical coordinates, and closes without adoption.

- [ ] **Step 4: Implement two explicit confirmations for motion**

The move flow must:

1. connect read-only and print current logical status;
2. compute whether the request is toward or away from the chip from immutable `StageStatus.side`, `toward_chip_sign`, `toward_chip_limit_um`, and `other_limit_um` fields;
3. print side, logical axis, signed um, classification, nominal/measured source, estimated voltage delta, hard limit, and `No automatic return or rollback`;
4. require exact input `ADOPT <LEFT|RIGHT>` before calling `adopt_baseline(confirm=True, allow_nominal=parsed_allow_nominal)`;
5. reprint the planned movement after adoption;
6. require exact input `MOVE <LEFT|RIGHT> <X|Y|Z> <signed-value> UM` before `move_by_um()`;
7. print requested and estimated completed displacement plus final logical voltages;
8. close in an operation-wide `BaseException` cleanup boundary.

The signed-value text in the required confirmation is generated once with `repr(parsed_float)` and reused in both disclosure and comparison. A return movement is a separate later invocation with the opposite signed displacement and a new pair of confirmations.

- [ ] **Step 5: Write moving, decline, failure, and cleanup precedence tests**

Test exact confirmation success, one-character mismatch, state changes between confirmations, nominal permission, measured calibration, toward/away disclosure on both sides, motion failure without false success text, body error plus close error, and close-only error. Assert outputs never mention controller X/Y mapping as physical meaning.

- [ ] **Step 6: Run diagnostic GREEN and help check**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberDiagnosticTests -v
conda run --no-capture-output -n VISA python -m Code.Debugs.check_fiber_stages --help
```

Expected: all focused tests pass; help exits 0 without enumerating hardware.

- [ ] **Step 7: Record Task 6 checkpoint and review**

AST-audit `check_fiber_stages.py` imports to prove it does not import the MDT driver or raw serial API. Verify tests patch every discovery/connection entry so no real port enumeration occurs. Suggested Git commit: `feat: add staged fiber stage diagnostic`.

---

### Task 7: Persistent Workspace Boundary and Experiment Import Guard

**Files:**
- Modify: `AGENTS.md`
- Modify: `Code/Debugs/test_fiber_coupling.py`
- Verify: `Code/Experiments/**/*.py`

**Interfaces:**
- Consumes: final public setup surface from Tasks 1-6.
- Produces: persistent human/AI rules and an AST regression that prevents experiment-level direct MDT use.

- [ ] **Step 1: Write the AST boundary test first**

Add `FiberWorkspaceBoundaryTests` that walks Python files under `Code/Experiments`, parses each with `ast.parse`, and fails on:

- `import Code.Utils.mdt693b`;
- any `from Code.Utils.mdt693b import <symbol>` form;
- importing `MDT693B` from `Code.Utils`;
- any `Name` or `Attribute` call whose constructor name is `MDT693B`.

Also inspect the public names exported by `Code.Setups` and prove neither `MDT693B`, `Axis`, nor a raw driver getter is exported.

```python
def test_experiments_do_not_bypass_fiber_setup_with_mdt693b(self):
    violations = scan_experiment_mdt_imports(Path("Code/Experiments"))
    self.assertEqual(violations, [])
```

Implement the scanner in the test module, not production.

- [ ] **Step 2: Run boundary test before documentation change**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberWorkspaceBoundaryTests -v
```

Expected: current experiments pass if they do not use MDT directly; exported setup-surface assertions may fail until `__all__` is final. For mutation evidence, temporarily add a synthetic AST source string containing each forbidden form and prove the scanner catches it; do not modify an experiment file.

- [ ] **Step 3: Add exact persistent rules to `AGENTS.md`**

Append a `### Fiber Coupling Setup` subsection under hardware safety containing these binding statements:

```markdown
- Control the two NanoMax fiber stages through `Code/Setups/fiber_coupling.py`; experiment code must not instantiate or call `MDT693B` directly.
- Bind the left controller to serial `2110148249-10` and the right controller to serial `160721175410`; never infer side from COM-port order.
- Use laboratory coordinates: +X right, +Y away from the operator, +Z up. The left setup maps logical X/Y/Z to MDT Y/X/Z; the right maps to MDT X/Y/Z.
- Treat stage position as a session-only open-loop estimate. Nominal MAX312D conversion requires explicit authorization and is not measured displacement.
- Limit each toward-chip move to 0.2 um and every other per-axis move to 1.0 um. Configuration may lower but never raise these limits.
- On authority loss, external/manual movement, or partial failure, stop and hold, invalidate the estimate, and require explicit baseline re-adoption. Never automatically roll back or zero either stage.
```

Retain all existing MDT driver and diagnostic staging rules.

- [ ] **Step 4: Run boundary and setup tests GREEN**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling.FiberWorkspaceBoundaryTests Code.Debugs.test_fiber_coupling -v
```

Expected: all pass with no experiment file modification.

- [ ] **Step 5: Record Task 7 checkpoint and review**

Compare every new `AGENTS.md` statement with the approved spec. Verify no rule says the estimates are measured position or allows implicit nominal motion. Suggested Git commit: `docs: enforce fiber stage setup boundary`.

---

### Task 8: Acceptance Matrix, Full Offline Verification, and Handoff

**Files:**
- Modify only if a verification failure reveals a requirement defect: `Code/Setups/fiber_coupling.py`, `Code/Debugs/test_fiber_coupling.py`, `Code/Debugs/check_fiber_stages.py`, `Code/Setups/__init__.py`, `Config/fiber_coupling.json`, or `AGENTS.md`
- Verify: complete workspace test suite and thread/resource audit

**Interfaces:**
- Consumes: all Tasks 1-7.
- Produces: verified implementation evidence and a hardware-stage handoff without opening real devices.

- [ ] **Step 1: Run the focused setup suite fresh**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_fiber_coupling -v
```

Expected: all tests pass, no errors/skips unless an existing platform-specific skip is already documented, and no real port enumerator is called.

- [ ] **Step 2: Run existing MDT and driver regressions**

```powershell
conda run --no-capture-output -n VISA python -m unittest Code.Debugs.test_mdt693b Code.Debugs.test_drivers -v
```

Expected: all pass. This confirms the setup did not alter generic MDT or shared-driver behavior.

- [ ] **Step 3: Run full workspace discovery**

```powershell
conda run --no-capture-output -n VISA python -m unittest discover -s Code/Debugs -p "test*.py" -v
```

Expected: all workspace tests pass.

- [ ] **Step 4: Compile and inspect both CLIs without hardware**

```powershell
conda run --no-capture-output -n VISA python -m py_compile Code/Setups/__init__.py Code/Setups/fiber_coupling.py Code/Debugs/check_fiber_stages.py Code/Debugs/test_fiber_coupling.py
conda run --no-capture-output -n VISA python -m Code.Debugs.check_fiber_stages --help
conda run --no-capture-output -n VISA python -m Code.Debugs.check_mdt --help
```

Expected: every command exits 0. `--help` must not enumerate or connect hardware.

- [ ] **Step 5: Run an in-process leak audit**

Use a `VISA`-environment one-liner or small test runner that loads and runs `Code.Debugs.test_fiber_coupling`, then examines `threading.enumerate()`. Assert no live thread name contains `FiberStage`, `FiberCoupling`, or `MDT693BMonitor`. Inspect all fake driver instances and assert every instance owned by a closed setup received close and retained no open resource.

- [ ] **Step 6: Audit the acceptance matrix manually**

Check each item against a named test and production path:

| Requirement | Required evidence |
|---|---|
| serial role binding | left-only/right-only/both/duplicate/identity-mismatch tests |
| unknown device untouched | factory construction count remains zero |
| read-only connection | zero fake setters in connect/read-only tests |
| logical mapping | exact left swap/right identity tests |
| nominal gate | adoption and diagnostic zero-write rejection tests |
| directional calibration | twelve unique-coefficient tests |
| 0.2/1.0 hard limits | exact and adjacent-float boundary tests |
| complete vector preflight | invalid last axis with zero setters |
| execution order | away X/Y/Z and toward Y/Z/X call logs |
| partial failure | no remaining setter, rollback, or zero; estimate unknown |
| independent sides | one-side failure leaves other call log/status unchanged |
| no direct MDT experiment API | AST boundary and `Code.Setups.__all__` tests |
| cleanup | context/close failure precedence and leak audit |
| diagnostic staging | enumerate/read-only/move parser and prompt tests |

If any row lacks deterministic evidence, add a focused failing test, capture RED, apply the smallest fix, and rerun the affected task plus the full matrix.

- [ ] **Step 7: Perform mutation sensitivity checks**

Temporarily and one at a time mutate: left X/Y mapping, right toward-chip sign, 0.2 limit, nominal authorization check, preflight-before-setter order, toward-X execution order, failure invalidation, and close-intent recheck. Each mutation must make a named focused test fail. Restore the source after every mutation and rerun the focused tests GREEN. Do not leave mutation files or cached artifacts in the source tree.

- [ ] **Step 8: Prepare the non-hardware handoff**

Report exact fresh test counts and commands, files created/modified, any preserved unrelated workspace changes, and the fact that no serial port or hardware was accessed. State that the next possible actions remain separately authorized:

1. `--enumerate` only;
2. read-only setup connection;
3. an explicitly approved small away-from-chip nominal move.

Do not run any of those stages as part of this implementation plan.

- [ ] **Step 9: Record final checkpoint**

In the current non-Git workspace, do not initialize Git. If execution occurs in a Git-backed copy and all verification is fresh, suggested final commit message: `feat: add calibrated dual fiber stage setup`.

## Execution Notes

- The implementation should use test-driven development for every behavior change: focused RED, minimal GREEN, then relevant regression.
- Each task is an independent review gate. Do not start the next task while the current task has an unresolved Critical or Important review finding.
- Hardware validation is not an implementation shortcut. Fake-only tests must establish configuration, mapping, limit, authority, failure, cleanup, and CLI contracts first.
- Do not add generic synchronization, absolute motion, persisted position, automatic rollback, automatic zero, calibration measurement, or experiment orchestration in this plan.
