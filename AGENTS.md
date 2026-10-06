# Workspace Rules

## Structure

- Put reusable instrument drivers in `Code/Utils`.
- Put hardware bring-up and diagnostic scripts in `Code/Debugs`.
- Put each experiment in `Code/Experiments/<name>`.
- Put each experiment's generated output in `Result/<name>`.
- Treat `reference_code` as read-only protocol reference.
- Keep file and experiment names short and descriptive.

## Driver Boundary

- Experiment code must use `Code/Utils` drivers; do not open serial ports or VISA resources directly from experiment code.
- Do not bypass voltage, current, temperature, startup, interlock, watchdog, or shutdown limits enforced by a driver.
- Do not put generated data, caches, or temporary files in `Code/Utils`.

## Hardware Safety

- Begin hardware diagnostics with identity and read-only checks.
- Clearly disclose and confirm any diagnostic that can change an output state before running it.
- Voltage Source commands are limited to 0-14 V. Normal changes use steps no larger than 0.1 V with at least 50 ms between steps. Handled failures and shutdown send all channels to 0 V immediately.
- Gain Driver commands are limited to 0-200 mA and target temperatures of 15-40 degC. Shutdown disables current before TEC.
- Gain current may be enabled only while TEC is on and measured temperature has remained within target +/-0.2 degC for five consecutive seconds.
- OSA acquisition preserves front-panel measurement settings.

### PM400

- PM400 connection and normal close must preserve all front-panel measurement settings.
- Capability-gate every sensor-dependent operation before any write. A duplicate session to the same canonical VISA resource must fail, while different VISA resources remain independent.
- Reset, zero/calibration, detector-response, and adapter-type changes require explicit operator authorization. Do not bypass the driver with raw SCPI or direct VISA access.

### MDT693B

- MDT693B connection and normal close are read-only and preserve every setting and output. The project output ceiling is 75 V.
- Normal voltage changes use steps no larger than 0.1 V with at least 0.050 s between steps.
- A fault is stop-and-hold: never automatically zero the piezo. Only an explicit `ramp_to_zero()` or `emergency_zero(confirm=True)` may command zero.
- Report external or manual residual voltage truthfully. Full recovery is read-only. Do not bypass the driver with raw serial data or arrow sequences.
- Read-only connect and recover leave absolute MDT motion disarmed; only a fully verified emergency zero or explicit operator-attested baseline adoption may establish axis-command authority.

### Fiber Coupling Setup

- Control the two NanoMax fiber stages through `Code/Setups/fiber_coupling.py`; experiment code must not instantiate or call `MDT693B` directly.
- Bind the left controller to serial `2110148249-10` and the right controller to serial `160721175410`; never infer side from COM-port order.
- Use laboratory coordinates: +X right, +Y away from the operator, +Z up. The left setup maps logical X/Y/Z to MDT Y/X/Z; the right maps to MDT X/Y/Z.
- Treat stage position as a session-only open-loop estimate. Nominal MAX312D conversion requires explicit authorization and is not measured displacement.
- Limit each toward-chip move to 0.2 um and every other per-axis move to 1.0 um. Configuration may lower but never raise these limits.
- On authority loss, external/manual movement, or partial failure, stop and hold, invalidate the estimate, and require explicit baseline re-adoption. Never automatically roll back or zero either stage.

### Diagnostic Staging

- Hardware diagnostics proceed as three separately authorized stages: enumeration, read-only connection, then an explicitly approved reversible action. Never infer authorization for a later stage from approval of an earlier one.

## Verification

- Run every Python command in the Anaconda `VISA` environment.
- Source development uses Python3.10.16 in `VISA`; follow `docs/development.md`. Keep the reviewed direct dependency pins in `requirements.txt` and `environment.yml` aligned and run `Code.Debugs.test_environment_docs` after changing either. Never automatically replace an existing environment.
- The sole deployment exception is explicitly approved qualification of the packaged private runtime: run that exact verified executable to prove independence from Anaconda. This does not waive offline testing, driver limits or staged hardware authorization, and is not a user-selectable alternate instrument backend.
- Run offline tests in the Anaconda `VISA` environment before touching hardware.
- Use the scripts under `Code/Debugs` for real-device validation.
- Always release serial/VISA resources in `finally` blocks or context managers.
- Shared VISA leases and resource reservations protect cooperating sessions only inside one Python process. An external owner must keep a borrowed manager alive until every dependent driver has confirmed release.
- Treat cleanup reports as immutable evidence from one attempt. Resource release, a successful lifecycle call, and host-observed voltage-zero evidence are separate claims; none substitutes for a physical measurement.
- App and experiment entry points are real-only; obsolete backend flags are rejected before activation. Historical synthetic data remains read-only and must never be resumed or relabeled as hardware acquisition.
- Hardware-free protocol, interlock, lifecycle and storage tests use explicitly injected bounded transport doubles, never a selectable instrument backend or GUI readings. These checks remain mandatory before real hardware diagnostics.
- Experiment execution opens real instruments and requires the applicable staged authorizations above; `--plan` only validates and previews configuration without opening devices.
