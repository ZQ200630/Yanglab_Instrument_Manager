# Dual Fiber Stage Setup Design

## Purpose

Add a setup-specific control layer for the two Thorlabs NanoMax 300 fiber-coupling stages. The layer converts laboratory-frame relative displacements in micrometres into safe MDT693B voltage ramps without exposing controller-axis names or raw MDT693B access to experiment code.

This is an open-loop positioning abstraction. It reports estimated relative displacement and calibration provenance; it never claims measured or absolute physical position.

All Python commands and tests run in the Anaconda `VISA` environment. Implementation and offline verification do not access hardware. Real-device checks follow the workspace's separately authorized enumeration, read-only connection, and reversible-action stages.

## Binding Hardware and Coordinates

The temporary stage model is Thorlabs `MAX312D`. Thorlabs specifies nominal 20 um open-loop piezo travel over 0-75 V for this model, corresponding to a nominal conversion of:

```text
20 um / 75 V = 0.2666666666666667 um/V
```

This value is an estimate only. Open-loop hysteresis, creep, loading, external inputs, and manual changes prevent it from establishing measured displacement. The relevant primary source is the [Thorlabs Motion Control catalog](https://www.thorlabs.com/images/Catalog/V21/V21_2_MotionControl.pdf).

The laboratory coordinate system is identical for both stages:

- `+X`: toward the operator's right.
- `+Y`: away from the operator, toward the rear of the optical table.
- `+Z`: upward.

The setup binds physical roles by exact controller serial number, never by COM-port number:

| Setup side | MDT693B serial number | Logical X | Logical Y | Logical Z | Toward chip |
|---|---|---|---|---|---|
| Left | `2110148249-10` | MDT Y, positive | MDT X, positive | MDT Z, positive | `+X` |
| Right | `160721175410` | MDT X, positive | MDT Y, positive | MDT Z, positive | `-X` |

Controller-axis names are configuration and diagnostic implementation details. Public setup status, commands, results, and exceptions use only logical `X`, `Y`, and `Z`.

## Chosen Architecture

Use a dedicated setup layer above the reusable MDT693B driver:

```text
Experiment code
    -> Code.Setups.fiber_coupling
        -> Code.Utils.mdt693b
            -> serial transport
```

Experiment code may not instantiate, import, or call `MDT693B` directly. The setup object owns each underlying MDT693B instance and does not expose it through a public property, escape hatch, or raw command method.

The alternatives were rejected:

- A generic coordinate-transform framework in `Code/Utils` adds abstraction that this single setup does not yet need and weakens the driver/setup boundary.
- Per-experiment axis mapping duplicates safety-critical serial, side, direction, calibration, and limit logic.

## Project Layout

```text
Code/
├── Setups/
│   ├── __init__.py
│   └── fiber_coupling.py
└── Debugs/
    ├── check_fiber_stages.py
    └── test_fiber_coupling.py
Config/
└── fiber_coupling.json
docs/superpowers/
├── specs/
│   └── 2026-08-20-dual-fiber-stage-setup-design.md
└── plans/
    └── 2026-08-20-dual-fiber-stage-setup.md
```

`AGENTS.md` will be extended with the setup boundary and safety rules. Existing generic MDT693B behavior remains in `Code/Utils/mdt693b.py`; setup identity, geometry, and displacement calibration must not be added to that driver.

## Components and Public Types

### `FiberCouplingSetup`

`FiberCouplingSetup` discovers registered MDT693B serial numbers, connects every registered stage currently present, owns resource cleanup, and exposes two stable side handles:

```python
with FiberCouplingSetup.connect() as setup:
    setup.left.status
    setup.right.status
```

At least one registered stage must be found. Either side may be absent. `setup.left` and `setup.right` always exist as handles, so callers never substitute one side for another or handle a nullable stage object.

The setup reports:

- `available_sides`;
- `missing_sides`;
- exact serial number and current serial resource for every available side;
- unknown MDT693B devices observed during enumeration.

An unknown MDT693B is reported but never opened or written. It does not block registered devices from operating. A duplicate registered serial number, ambiguous identity, identity-query failure, or attempt to bind one physical resource to both sides fails closed before movement.

COM ports are resolved for the current session only. The setup does not silently reconnect a lost side on another port during an active session.

### `FiberStage`

Each side handle supplies the relative-motion surface:

```python
stage.adopt_baseline(confirm=True, allow_nominal=True)
stage.move_by_um(dx=0.05, dy=0.0, dz=0.02)
stage.status
```

Version one intentionally has no absolute-position method. There is no `move_to_um()`.

### Immutable public values

The module defines immutable public values for:

- `StageSide`: `LEFT` or `RIGHT`.
- `LogicalAxis`: `X`, `Y`, or `Z`.
- `Vector3Um`: finite logical X/Y/Z values in micrometres.
- `StageStatus`.
- `MoveResult`.
- validated calibration metadata.

`StageStatus` includes:

- side and availability;
- registered serial number and current resource when connected;
- connection/fault/restriction state;
- whether a session baseline is known;
- estimated session-relative position or `None` when unknown;
- observed controller outputs mapped into logical X/Y/Z keys;
- active calibration source and date;
- whether nominal calibration has been explicitly authorized for the current baseline.

`MoveResult` includes:

- requested logical displacement;
- calibration coefficient selected for each nonzero direction;
- planned voltage delta in logical coordinates;
- estimated position before and after the move;
- final observed voltages in logical coordinates;
- calibration provenance;
- complete confirmation status.

The values are estimates and command evidence, not metrology results.

## Connection and Availability

Connection performs discovery and identity/read-only connection only. It does not adopt a baseline, alter a voltage, restore a prior estimate, change Master Scan, or zero a controller.

Availability behavior is explicit:

- A present registered side is connected and represented by an available handle.
- A missing registered side remains represented by an unavailable handle.
- A motion or baseline operation on an unavailable handle raises `StageUnavailableError` without transport I/O.
- Finding neither registered side raises `StageConnectionError` and releases enumeration/connection resources.
- If connection of one discovered registered side fails, the setup closes any side it already opened and does not publish a partially successful setup object.

The final rule prevents a connection error from being mistaken for an intentionally absent side. Intentional one-side operation is supported only when the other registered controller was not discovered.

## Session Baseline and MDT Authority

Estimated stage position and baseline exist only for the current connection session. They are never persisted to disk or restored after reconnect, even after a normal prior shutdown.

`FiberStage.adopt_baseline(confirm=True, allow_nominal=...)`:

1. Requires an available, healthy side.
2. Calls the underlying explicit operator-attested MDT baseline adoption; it never fabricates a command baseline from observed voltage.
3. Requires Master Scan disabled and programmed to zero as enforced by the driver.
4. Obtains a fresh public MDT status after successful adoption and maps its confirmed actual outputs into a setup-owned session command baseline. The setup never reads an MDT private command cache.
5. Defines the current physical state as estimated logical `(0, 0, 0)` for this session.
6. Records calibration provenance and, when applicable, session authorization to use nominal MAX312D calibration.

When any active calibration coefficient has `source="nominal_MAX312D"`, adoption requires `allow_nominal=True`. That explicit authorization lasts only for the adopted baseline and is cleared whenever the baseline becomes unknown. A later measured calibration does not require the nominal authorization flag.

Movement is rejected before a setter unless both the setup baseline and underlying `MDTStatus.axis_command_known` are true.

Any of the following invalidates the entire side's estimated position and nominal authorization:

- unexplained actual-voltage movement detected by a getter or monitor;
- underlying loss of axis-command authority;
- a partially completed or unconfirmed motion;
- transport or protocol failure during motion;
- manual or external-input interference;
- reconnect, recovery, or factory restoration;
- cleanup whose physical result cannot be confirmed.

After invalidation, status remains readable but displacement motion is blocked until a fresh explicit adoption.

## Calibration Model

Calibration is independent for every side, logical axis, and direction:

```text
left.x.positive_um_per_v
left.x.negative_um_per_v
...
right.z.positive_um_per_v
right.z.negative_um_per_v
```

There are twelve coefficients. Each coefficient has:

- a finite positive `um_per_v` value;
- `source`, initially `nominal_MAX312D`;
- optional calibration date and note.

Version one initializes every direction to `0.2666666666666667 um/V`. The data model is already direction-specific so later measured hysteresis calibration replaces individual coefficients without changing the public motion API.

For each requested logical displacement:

```text
delta_controller_v = logical_delta_um / selected_um_per_v / polarity
```

The selected coefficient follows the logical displacement sign. The configured polarity maps logical positive displacement to controller voltage direction. In the approved setup all six logical-axis polarities are positive; the left side additionally swaps logical X/Y to controller Y/X.

Only confirmed command evolution updates estimated position. The setup does not infer physical displacement by multiplying arbitrary observed voltage changes, because external/manual contributions may be present.

The setup computes each later absolute controller target from its own confirmed session command baseline plus the requested calibrated voltage delta. After every successful ramp it replaces that baseline with the public confirmed target/observation evidence. If the public observation is inconsistent with the expected result, the move fails and the baseline becomes unknown instead of being adjusted to fit the observation.

## Configuration and Validation

`Config/fiber_coupling.json` contains:

- registered side and serial identities;
- stage model;
- logical-to-controller axis mapping and polarity;
- twelve directional calibration records;
- optional operator limits that are equal to or more restrictive than code hard limits.

Configuration is loaded and completely validated before opening a registered device. Validation requires:

- exactly the `left` and `right` registered roles;
- nonempty unique serial numbers;
- each side's controller-axis mapping to be a one-to-one permutation of X/Y/Z;
- polarity values to be exactly `+1` or `-1`;
- finite positive calibration coefficients;
- recognized calibration-source metadata;
- configured movement limits not to exceed code hard limits;
- exact `MAX312D` model in this first implementation.

Malformed, incomplete, duplicated, non-finite, or unsafe configuration raises a typed configuration/safety error before serial enumeration or write activity.

## Relative-Motion Safety

All displacement inputs must be real, finite, and non-boolean. Version one enforces immutable per-call hard limits:

- toward-chip logical X movement: at most `0.2 um`;
- away-from-chip logical X movement: at most `1.0 um`;
- logical Y and Z movement in either direction: at most `1.0 um` per axis.

Toward-chip direction is side-specific:

- left: positive logical X;
- right: negative logical X.

The hard limits cannot be increased by configuration or a call argument. Configuration may reduce them. An over-limit request is rejected rather than automatically split, forcing experiment logic to make repeated approach steps explicit.

Before the first setter, a move performs one complete preflight over all requested axes:

- side available and healthy;
- session baseline known and underlying command authority known;
- nominal calibration authorized when used;
- displacement and calibration values valid;
- every per-axis displacement within its applicable hard/configured limit;
- every derived controller target within 0-75 V, hardware limits, application limits, and current controller axis limits;
- no detected manual/external change since the confirmed baseline;
- no close or cancellation intent.

If any axis fails preflight, the whole call sends zero motion setters.

The setup delegates voltage ramping, confirmation, authority checks, cancellation, and the maximum 0.1 V step/minimum 50 ms interval to the MDT693B driver. It does not reproduce or bypass driver safety logic.

## Multi-Axis Execution Order

The MDT693B cannot execute arbitrary independent XYZ displacement targets atomically. `move_by_um(dx, dy, dz)` therefore preflights the entire vector and then performs confirmed single-axis ramps in a collision-conscious sequence:

- If logical X moves away from the chip, execute X first, then Y, then Z.
- If logical X moves toward the chip, execute Y, then Z, and X last.
- If logical X is zero, execute Y then Z.
- Zero-displacement axes are skipped.

Every axis ramp uses the controller axis selected by the approved side mapping. The setup updates its public estimated position only after the complete vector succeeds.

If any ramp fails after an earlier axis moved, the setup:

- sends no remaining motion command;
- performs no compensating rollback and no automatic zero;
- marks the side's estimated position and baseline unknown;
- preserves the requested vector, completed-operation evidence, last observed logical voltages, and underlying exception;
- leaves the other side untouched.

No method claims that a two-stage or three-axis operation is atomic.

## Independent Left and Right Operation

Left and right moves are independent public operations. The setup does not provide a fake synchronized move in version one.

Experiment code determines ordering between the stages. A failure on one side neither moves nor rolls back the other side. If an experiment needs a coupled procedure later, it must define explicit ordering, stop conditions, and partial-completion behavior above these single-side primitives.

Per-side operation locks prevent two motions from interleaving on one stage. Operations on different sides may be issued independently, but the first implementation does not promise cross-controller timing alignment.

## Errors and Cleanup

The setup defines typed high-level errors, retaining the relevant underlying driver error as `__cause__` where applicable:

- `StageUnavailableError`;
- `BaselineUnknownError`;
- `CalibrationRequiredError`;
- `StageMotionLimitError`;
- `StageConnectionError`;
- `StageMotionError`.

Local validation, unavailable-side, calibration, and preflight failures occur before a setter and do not invalidate a still-confirmed baseline unless new observation proves it stale.

An attempted, partial, cancelled, timed-out, or unconfirmed motion invalidates the side baseline and records fault evidence. The earliest operation exception remains primary; cleanup errors are retained as secondary evidence.

Normal close releases monitor and serial resources through the underlying drivers without changing voltage. Close never persists estimated position. Failure to close one side does not hide the result for the other side, and the setup does not claim complete disconnection while an owned resource remains open.

Context-manager cleanup preserves a context-body exception as primary and attaches close failures as cleanup evidence.

## Hardware Diagnostic

`Code/Debugs/check_fiber_stages.py` exposes three separately authorized stages:

```powershell
conda run -n VISA python -m Code.Debugs.check_fiber_stages --enumerate
conda run -n VISA python -m Code.Debugs.check_fiber_stages --read-only
conda run -n VISA python -m Code.Debugs.check_fiber_stages --move ...
```

### Enumeration

- Enumerates candidate MDT693B serial resources.
- Identifies registered left/right roles by exact serial number.
- Reports registered missing sides and unknown MDT devices.
- Opens no instrument and changes no output.

### Read-only connection

- Connects available registered sides through the setup layer.
- Displays side, serial, resource, logical X/Y/Z observed voltages, calibration provenance, baseline state, and restrictions.
- Does not adopt a baseline, change Master Scan, or move.

### Reversible motion

- Requires separate operator approval after offline and read-only stages succeed.
- Uses only logical side/axis/displacement arguments.
- Discloses exact side, direction, requested displacement, estimated voltage change, calibration source, and restoration plan before confirmation.
- Requires `--allow-nominal` when nominal calibration is active.
- Starts with a separately approved small away-from-chip displacement.
- Performs any return movement only under a separate explicit approval; it never treats open-loop return as proof of restored physical position.
- Closes while preserving the final confirmed controller output.

No real hardware action is authorized by implementation of this design.

## Offline Verification

All new tests run under `conda run -n VISA` with injected discovery and fake MDT693B instances. Coverage includes:

- left-only, right-only, and both-present discovery;
- neither-present failure and cleanup;
- unknown MDT report-without-open behavior;
- duplicate/ambiguous serial rejection;
- exact left axis swap, right identity mapping, and all approved polarities;
- stable unavailable-side handles and zero-I/O errors;
- read-only connection and close;
- no baseline restoration across sessions;
- explicit baseline adoption and nominal-authorization lifecycle;
- twelve directional calibration selections;
- invalid, missing, duplicate, non-finite, negative, and unsafe configuration;
- left `+X` and right `-X` toward-chip classification;
- exact hard-limit boundaries and just-over-boundary rejection;
- target voltage, hardware, application, and controller-axis limit intersections;
- complete-vector zero-write preflight;
- away-X-first and toward-X-last execution order;
- no-op vector behavior;
- successful immutable status/result publication;
- partial vector failure, cancellation, timeout, manual/external deviation, authority loss, and baseline invalidation;
- no automatic rollback or zero;
- independent left/right failure behavior;
- cleanup error precedence and no serial/monitor-thread leak;
- absence of a public raw MDT escape hatch;
- static enforcement that `Code/Experiments` does not import `Code.Utils.mdt693b` or instantiate `MDT693B`.

Existing PM400, MDT693B, OSA, voltage-source, gain-driver, and experiment tests remain green.

## Persistent Workspace Rules

`AGENTS.md` will add:

- The two NanoMax fiber stages are controlled through `Code/Setups/fiber_coupling.py`, not directly through `MDT693B` from experiment code.
- Left/right identity is established by the two approved serial numbers, never COM-port ordering.
- Experiment coordinates are `+X` right, `+Y` away from the operator, and `+Z` up.
- The left side swaps controller X/Y; the right side does not. Public experiment APIs do not use controller-axis meaning.
- Stage positions are open-loop session estimates; never describe them as measured absolute position.
- Nominal MAX312D displacement conversion requires explicit operator authorization.
- Toward-chip moves are limited to 0.2 um per call; all other per-axis moves are limited to 1.0 um per call.
- Baseline loss, partial motion, or manual/external movement blocks further displacement commands until explicit re-adoption.
- Faults stop and hold. They do not automatically roll back or zero either stage.
- Hardware diagnostics remain separately authorized enumeration, read-only, and reversible-action stages.

## Acceptance Criteria

- Experiments can address either available side by physical role and logical displacement without controller-axis names.
- Left/right binding uses exact registered serial numbers and tolerates either registered side being absent.
- Unknown MDT devices are reported and untouched.
- Read-only connection changes no controller output or setting.
- Experiment code has no supported direct MDT693B or raw serial escape path.
- The approved logical coordinate mapping and toward-chip directions are exact.
- Version one supports relative motion only.
- A session baseline is explicit, nonpersistent, and dependent on underlying MDT axis-command authority.
- Nominal calibration cannot move a stage without explicit session authorization.
- Every side/axis/direction has an independent calibration coefficient and provenance.
- All motion is preflighted as a vector before any setter.
- Toward-chip and general per-call hard limits cannot be raised by configuration.
- Derived targets never bypass the MDT driver's 0-75 V, configured-limit, 0.1 V-step, 50 ms-spacing, confirmation, cancellation, and authority enforcement.
- Multi-axis order is away-X first or toward-X last as applicable.
- Partial failure stops remaining work, performs no rollback/zero, and invalidates the side estimate.
- One side's failure causes no automatic output change on the other side.
- Status and results clearly label estimates and calibration sources.
- Offline tests and the existing workspace suite pass in `VISA` before any hardware action.
- The diagnostic enforces separate enumeration, read-only, and explicit reversible-action approvals.

## Operational Limits

This layer does not measure physical displacement. It cannot eliminate open-loop hysteresis, creep, mechanical backlash, mounting error, calibration drift, external analog contributions, panel interaction, fiber/chip collision risk, host termination, USB loss, controller firmware faults, or changes that occur after the last confirmed query.

The 0.2 um toward-chip limit is a per-call software guard, not collision detection. Repeated commands can accumulate motion. Operator observation, conservative step selection, physical travel limits, and later metrology-based calibration remain required safety layers.
