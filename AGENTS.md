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

- Control the two NanoMax fiber stages through the Rust `yang-setups` crate (`Code/Setups/src/fiber.rs`); experiment code must not instantiate or call `MDT693B` directly.
- Bind the left controller to serial `2110148249-10` and the right controller to serial `160721175410`; never infer side from COM-port order.
- Use laboratory coordinates: +X right, +Y away from the operator, +Z up. The left setup maps logical X/Y/Z to MDT Y/X/Z; the right maps to MDT X/Y/Z.
- Treat stage position as a session-only open-loop estimate. Nominal MAX312D conversion requires explicit authorization and is not measured displacement.
- Limit each toward-chip move to 0.2 um and every other per-axis move to 1.0 um. Configuration may lower but never raise these limits.
- On authority loss, external/manual movement, or partial failure, stop and hold, invalidate the estimate, and require explicit baseline re-adoption. Never automatically roll back or zero either stage.

### Diagnostic Staging

- Hardware diagnostics proceed as three separately authorized stages: enumeration, read-only connection, then an explicitly approved reversible action. Never infer authorization for a later stage from approval of an earlier one.

## Interaction Responsiveness

- Treat interaction speed and visible waiting feedback as design requirements for every future App feature, not optional polish.
- A potentially slow action must acknowledge the click immediately, explain what is pending, and show success, failure, cancellation or an uncertain outcome explicitly. Keep navigation and unrelated controls usable; disable only conflicting actions.
- Use honest progress: percentages only when backed by measured work or received bytes. Otherwise show an indeterminate indicator and elapsed time; long waits must not look like a frozen App. Never invent completion, a safe state or an estimated duration.
- Keep instrument I/O, network waits, storage and large data processing off the blocking interaction path. Bound work and queues, yield during large UI-side processing, and avoid copying immutable samples or rebuilding unchanged controls on every update. Preserve draft inputs, keyboard focus and scroll.
- Preserve previous data with explicit previous/historical labels while waiting; never present it as a newly completed measurement. A timeout must not automatically replay an instrument command. Cancellation may be claimed only at the boundary actually confirmed.
- Verify responsiveness using delayed/error/offline test cases and record stage timings before choosing a language rewrite as a performance fix.

## Verification

- Active driver, Worker, Host and diagnostic development uses Rust; run `App/scripts/test-native.ps1` before hardware work. Python/Anaconda is not an App/runtime/build prerequisite.
- Cargo.toml/Cargo.lock define native dependencies. Native test fixtures are development-only and must never ship or appear as catalog devices.
- Preserved Python drivers/workers/experiments and requirements.txt/environment.yml are legacy references only; do not auto-run or migrate them. If specifically inspecting legacy Python, run every Python command in Anaconda VISA and follow docs/development/legacy-python.md.
- Use the native `yang-debug` diagnostics under `Code/Debugs` for real-device validation; default invocation previews without enumeration or opening devices.
- Native resource owners must use RAII and explicit driver cleanup receipts; keep pending owners alive until actual release. Legacy reference code still requires finally/context-manager cleanup.
- Shared native VISA leases/resource reservations protect cooperating sessions inside one process, while the machine-wide Host/diagnostic guard and native exclusive leases add cross-process protection. Keep a shared manager alive until every dependent driver has confirmed release.
- Treat cleanup reports as immutable evidence from one attempt. Resource release, a successful lifecycle call, and host-observed voltage-zero evidence are separate claims; none substitutes for a physical measurement.
- App and experiment entry points are real-only; obsolete backend flags are rejected before activation. Historical synthetic data remains read-only and must never be resumed or relabeled as hardware acquisition.
- Hardware-free protocol, interlock, lifecycle and storage tests use explicitly injected bounded transport doubles, never a selectable instrument backend or GUI readings. These checks remain mandatory before real hardware diagnostics.
- Experiment execution opens real instruments and requires the applicable staged authorizations above; `--plan` only validates and previews configuration without opening devices.
