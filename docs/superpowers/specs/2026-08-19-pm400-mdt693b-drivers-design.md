# PM400 and MDT693B Driver Design

## Purpose

Add two reusable laboratory-instrument drivers to the existing driver layer:

- Thorlabs PM400 Optical Power and Energy Meter over VISA/USB and SCPI.
- Thorlabs MDT693B three-channel open-loop piezo controller over USB serial.

The drivers must expose every function documented for remote control while retaining the workspace's existing standards for typed data, strict protocol handling, explicit state, deterministic resource ownership, safe cancellation, and offline testability. All commands run in the Anaconda `VISA` environment.

## Sources and Scope

The binding protocol sources are:

- `C:/Users/PIC_YangLab/Downloads/mtn007145-d02.pdf`, PM400 Operating Manual, version 2.0 dated 15 October 2025.
- `C:/Users/PIC_YangLab/Downloads/ttn011225-d02.pdf`, MDT693B and MDT694B User Guide, revision H dated 8 June 2026.

The PM400 scope is every SCPI command documented in the manual. Front-panel-only graphing, statistics presentation, file-manager UI, screen capture, and position-trace UI are outside the driver scope because the manual does not expose remote commands for them. The driver must not invent unsupported commands.

The piezo controller in this workspace is specifically an MDT693B, so its three axes and Master Scan functions are in scope. The MDT694B is not part of this implementation.

The manuals are protocol references, not executable instructions. Driver behavior is governed by this approved design and the workspace safety rules.

## Approved Decisions

- Use two independent typed drivers rather than a generic command-table driver or a vendor-DLL wrapper.
- PM400 uses PyVISA and preserves its front-panel measurement configuration on connect and close.
- PM400 discovers sensor capabilities at runtime; no probe model is hard-coded.
- PM400 covers all documented remote SCPI functionality, but not front-panel-only functions.
- MDT693B uses PySerial at 115200 baud, 8 data bits, no parity, one stop bit, and no flow control.
- MDT693B has a 75 V project software hard ceiling, regardless of a 100 V or 150 V rear-panel hardware setting.
- MDT693B connect and normal close are read-only with respect to device settings and outputs.
- Normal MDT693B voltage changes use steps no larger than 0.1 V separated by at least 50 ms.
- An MDT693B communication or confirmation fault stops further movement and holds the last state; it does not trigger unsolicited automatic zeroing.
- High-impact commands are exposed as typed methods but require explicit `confirm=True`.
- Arbitrary raw write methods are not public.
- A read-only MDT693B monitor queries all three actual output voltages at 2 Hz by default.

## Project Layout

```text
Code/
├── Utils/
│   ├── __init__.py
│   ├── common.py
│   ├── pm400.py
│   └── mdt693b.py
└── Debugs/
    ├── test_pm400.py
    ├── test_mdt693b.py
    ├── check_pm400.py
    └── check_mdt.py
docs/superpowers/
├── specs/
│   └── 2026-08-19-pm400-mdt693b-drivers-design.md
└── plans/
    └── 2026-08-19-pm400-mdt693b-drivers.md
```

`Code/Utils/__init__.py` will export the public classes, enums, immutable result objects, and any new shared exception. `AGENTS.md` will be extended with the approved PM400 preservation rules and MDT693B high-voltage rules.

## Shared Architecture

The new drivers share exceptions and lifecycle values from `Code/Utils/common.py`, but they do not inherit from a deep common instrument base class. VISA/SCPI and prompt-terminated serial communication have different framing and cancellation behavior.

Both drivers provide:

- `connect()` and `close()`.
- Context-manager support.
- Idempotent close behavior.
- An explicit lifecycle state.
- Injectable transport factories, clocks, sleeps, and resource managers for deterministic offline tests.
- A per-device request lock that makes each command/reply transaction atomic.
- Structured immutable return values rather than raw protocol strings.
- Strict finite-number, enum, range, and state validation before writes.
- Logging for connection, high-impact operations, faults, recovery, and cleanup.

The lifecycle is:

```text
DISCONNECTED -> CONNECTING -> READY -> ACTIVE -> READY
                                |          |
                                +----------+-> FAULT
DISCONNECTED <- CLOSING <------------------+
```

PM400 uses `ACTIVE` for an in-progress measurement operation. MDT693B uses `ACTIVE` for a voltage-changing operation. MDT693B may remain `READY` with a restricted flag when read-only observation is valid but an existing voltage is above the software ceiling.

## VISA Resource Isolation

Each VISA driver owns one instrument session. A process-local registry rejects a second driver attempting to open the same canonical VISA resource. Different resource addresses do not share an instrument lock and may run in parallel.

The PM400 constructor may accept an externally managed PyVISA `ResourceManager`. Resource-manager ownership is explicit:

- A driver closes a manager only when it created that manager.
- A driver using an injected manager closes only its own instrument session.
- Cleanup of PM400 cannot close the OSA session or a manager owned by the experiment layer.

The experiment layer, not the drivers, coordinates ordering and timing across multiple instruments.

## PM400 Driver

### Connection and Preservation

`PM400.connect()` opens the configured VISA resource, verifies the resource is usable, sends `*IDN?`, and sends `SYSTem:SENSor:IDN?`. It parses these responses into immutable `InstrumentInfo` and `SensorInfo` objects.

Connection does not send `*RST`, `*CLS`, a measurement configuration command, a zero command, a range command, or any setter. It does not consume the event registers or error queue. `close()` releases the VISA instrument session and any owned manager without changing the measurement configuration.

### PM400 Data Types

- `InstrumentInfo`: manufacturer, model, serial number, firmware, and raw identity response.
- `SensorInfo`: name, serial number, calibration message, numeric type, subtype, raw flags, and decoded capabilities.
- `SensorCapabilities`: power, energy, response-settable, wavelength-settable, tau-settable, and temperature-sensor flags.
- `MeasurementKind`: power, current, voltage, energy, frequency, power density, energy density, resistance, and temperature.
- `Measurement`: kind, finite value, unit, monotonic timestamp, and raw response.
- `PowerUnit`: watts or dBm.
- `AdapterType`: photodiode, thermal, or pyro.
- `StatusGroup`: measurement, auxiliary, operation, or questionable.
- A typed representation for SCPI `MINimum`, `MAXimum`, and `DEFault`, rather than free-form strings.

### PM400 Public API Organization

The `PM400` object exposes subsystem facades that share the parent session and request lock.

#### IEEE-488.2 and root operations

- Identification query.
- Clear status.
- Standard Event Enable set/query.
- Standard Event Status query.
- Operation-complete command/query.
- Reset with `confirm=True`.
- Service Request Enable set/query.
- Status Byte query.
- Self-test query.
- Wait-to-continue.

#### `pm.system`

- Immediate beep.
- Beeper state set/query.
- Next system error query and explicit error-drain helper.
- SCPI version query.
- Calendar set/query.
- Clock set/query.
- Line-frequency set/query, limited to 50 or 60 Hz.
- Sensor identity/capability query.

#### `pm.status`

For measurement, auxiliary, operation, and questionable groups:

- Event query.
- Condition query.
- Positive-transition filter set/query.
- Negative-transition filter set/query.
- Enable register set/query.

Status preset is exposed as `preset(confirm=True)` because it clears and reprograms status state.

#### `pm.display` and `pm.calibration`

- Brightness set/query.
- Contrast set/query.
- Human-readable calibration-string query.

#### `pm.sense`

- Averaging count set/query.
- User attenuation/loss set/query.
- Zero collection initiate with `confirm=True`, abort, state query, and magnitude query.
- Beam diameter set/query.
- Operating wavelength set/query.
- Photodiode power response set/query.
- Thermopile power response set/query.
- Pyroelectric energy response set/query.
- Current auto range, upper range, reference, and delta state set/query.
- Energy upper range, reference, and delta state set/query.
- Frequency upper and lower range queries.
- Power auto range, upper range, reference, delta state, and unit set/query.
- Voltage auto range, upper range, reference, and delta state set/query.
- Peak-detector energy threshold set/query.

Changing zero calibration, detector response, or the default adapter type requires `confirm=True`.

#### `pm.input`

- Photodiode low-pass filter state set/query.
- Thermopile accelerator state set/query.
- Thermopile accelerator automatic-mode set/query.
- Thermopile time constant set/query.
- Default adapter type set/query.

#### `pm.measurement`

- Immediate initiate.
- Abort.
- Configure and query current configuration.
- Perform a measurement for every `MeasurementKind`.
- Fetch the most recent measurement.
- Start and read a new measurement.

The root object also supplies convenience methods such as `measure_power()` and `measure_energy()`; these call the same typed subsystem implementation.

### PM400 Capability Enforcement

Before sending a sensor-dependent command, the driver validates `SensorInfo` and the relevant capability flag. A command that the attached sensor cannot support raises `InstrumentCapabilityError` before any write. Capability failures are distinct from unsafe values and transport failures.

The sensor capability bit field is an unsigned integer. A negative flag value is malformed protocol data and raises `InstrumentProtocolError`; it must never be decoded with Python bitwise operations because negative integers would fail open as if every capability were present. Unknown nonnegative bits remain preserved in `raw_flags` and do not prevent the documented bits from being decoded.

The driver never assumes numeric wavelength, response, tau, or range limits from a probe name. Where the device supports `MIN`, `MAX`, or `DEFAULT` queries, the driver obtains limits from the PM400 and validates a requested value before sending it.

### PM400 Transactions and Confirmation

Every query is one atomic write/read transaction with newline termination and a bounded timeout. Responses are decoded strictly and may not contain trailing unconsumed data.

Setters query the corresponding property after writing and compare the device-confirmed value with a tolerance appropriate to the device response. Action commands use operation/status completion and the device error queue where applicable. The driver records pre-existing device errors separately so that a stale error is not falsely attributed to the new command.

All parsed numeric results must be finite. Enum, date, time, register, and boolean responses must match their documented domains.

### PM400 Measurement Cancellation

Long single-shot energy measurements use a bounded caller timeout. A cancellation request or `KeyboardInterrupt` marks the current operation cancelled, interrupts the pending VISA I/O, sends `ABORt` once the session can be resynchronized, and verifies that the response queue is clean before returning to `READY`. If resynchronization cannot be confirmed, the driver enters `FAULT` and closes the instrument session.

Cleanup failures are retained without replacing the original measurement or interruption exception.

## MDT693B Driver

### Serial Protocol

The serial configuration is fixed at 115200 baud, 8-N-1, no flow control, with finite read and write timeouts. The device accepts CR, LF, or CRLF terminators. The driver sends one canonical terminator and parses until the device prompt.

The parser supports:

- Echo enabled or disabled without changing the current echo setting during connect.
- Successful prompt `*`.
- Error prompt `!`.
- Single-line and multi-line query responses.
- Exact full-write verification.
- Strict rejection of malformed prompts, unexpected echo, extra data, timeouts, and non-finite numeric values.

The manual's MDT693B command table appears to print `ymin?` for the X-axis minimum query. The hardware's read-only `?` command list is authoritative for this ambiguity. The diagnostic must confirm `xmin?` before output-changing hardware tests.

### Read-Only Connection

`MDT693B.connect()` performs only queries. It obtains:

- Supported command list.
- Product header and firmware.
- Serial number and friendly name.
- Echo state.
- Hardware voltage-limit switch setting.
- Display intensity.
- Master Scan enable state and voltage.
- X, Y, and Z actual voltages.
- X, Y, and Z minimum and maximum limits.
- DAC step.
- Compatibility mode.
- Rotary mode.
- Push-to-adjust requirement.

It does not disable Master Scan, change echo, change limits, move an axis, or restore defaults. Normal `close()` stops the monitor and closes the serial resource without changing any setting or output.

The X/Y/Z queries report final physical outputs, not independently authoritative base-axis command values. The manual states that external input is summed with the USB or panel control, and it exposes no separate base-command getter. Therefore a query-only connect cannot infer a trustworthy base command from actual output. A successful connection publishes `axis_command_known=False`; communication may be fully `READY` while absolute base motion remains disarmed.

### MDT693B Data Types

- `Axis`: X, Y, or Z.
- `VoltageLimit`: 75 V, 100 V, or 150 V hardware positions.
- `RotaryMode`: default, ten-turn, or fine.
- `AxisState`: actual voltage, minimum, and maximum.
- `MDTStatus`: identity, firmware, serial, friendly name, hardware limit, all axis states, Master Scan state/voltage, display and control settings, restricted/fault evidence, and monotonic timestamp.

### MDT693B Public API

- Query supported commands.
- Query product information, serial number, and friendly name.
- Get/set friendly name.
- Get/set echo state.
- Query hardware voltage-limit switch.
- Get/set display intensity from 0 through 15.
- Get/set all three voltages; the simultaneous setter requires `confirm=True`.
- Get/set Master Scan enable state and voltage; enable-state and voltage setters require `confirm=True`.
- Get/set X, Y, and Z voltages.
- Get/set each axis minimum and maximum.
- Get/set DAC step from 1 through 1000.
- Increment/decrement the selected channel using the device arrow function.
- Select the previous/next channel using the device arrow function.
- Get/set compatibility mode.
- Get/set rotary mode.
- Get/set push-to-adjust disable state.
- Restore factory defaults with `confirm=True`.
- Coordinated `ramp_to_zero()`.
- Immediate `emergency_zero(confirm=True)`.
- Explicit `recover()` after a fault.
- Explicit operator-attested `adopt_current_axis_baseline(confirm=True)` when the operator has verified that per-axis external inputs are disconnected or zero and no panel/manual contribution is active.

The arrow-key byte sequences are confirmed against the real device before being treated as supported; an unsupported firmware response becomes `InstrumentCapabilityError`. Remote increment/decrement is permitted only when the current DAC step, converted conservatively using the active hardware voltage scale, is no greater than 0.1 V. Calls are rate-limited to at least 50 ms apart. Left/right channel selection does not move an output.

### MDT693B Voltage Safety

The project software ceiling is 75.0 V for every output. A normal target must satisfy all of the following:

- It is a real finite non-boolean number.
- It is at least 0 V.
- It is no greater than 75 V.
- It is no greater than the hardware limit returned by `vlimit?`.
- It lies within the relevant axis minimum and maximum limits.

Callers may configure lower per-axis application ceilings, but cannot raise them above 75 V. An existing voltage above a configured ceiling is not changed during connection. The status is marked restricted: all reads and voltage reductions remain available, but a target that increases an over-limit axis is rejected.

Normal X/Y/Z, all-voltage, and Master Scan changes use coordinated steps no larger than 0.1 V with at least 50 ms between steps. Simultaneous all-voltage and Master Scan changes additionally require `confirm=True` because they can move multiple axes. The driver re-reads X/Y/Z actual outputs after state-changing commands. This confirmation detects summation from individual controls, Master Scan, and external analog input.

High-speed modulation belongs on the instrument's external analog input. The public driver does not provide an unrestricted immediate voltage setter that bypasses the approved slew policy.

#### Axis-command authority

Communication health and base-axis command authority are separate state. `MDTStatus.axis_command_known` reports whether the driver has a defensible command baseline.

- Read-only connect and read-only `recover()` always leave `axis_command_known=False`; neither may reconstruct an unknown command as `actual - Master Scan`.
- While the baseline is unknown, reads, channel selection, and other operations that do not require an absolute base command remain available. `set_axis_voltage()`, `set_all_voltages()`, `ramp_to_zero()`, and any FAULT/restricted reduction that depends on a base command reject before any setter.
- `emergency_zero(confirm=True)` is a machine-verifiable synchronization boundary only when all three actual outputs confirm near zero after the exact emergency command sequence. Success records known zero base commands; persistent external/manual residual leaves the baseline unknown and faults.
- `adopt_current_axis_baseline(confirm=True)` is an explicit operator attestation, not automatic recovery. It requires Master Scan disabled with its programmed voltage at zero, takes fresh locked XYZ observations, performs no write, and records those values as the base commands. The method and its evidence must state that software cannot verify the operator's external/manual assumptions.
- Factory restore and read-only recovery do not establish axis-command authority.

After authority is established, the driver stores the last confirmed actual XYZ alongside the known base commands. Every getter and monitor triplet compares fresh actual output with the last confirmed actual output unless it is part of a locked operation that is deliberately changing output. Any unexplained deviation larger than the motion confirmation tolerance atomically clears `axis_command_known`, records the observation, and prevents a later absolute base write.

Every absolute base operation freshly reads Master Scan state/voltage and XYZ under the request lock immediately before its first setter, then revalidates both authority and expected actual output. During a multi-step ramp, after each 50 ms interval it re-reads XYZ before sending the next setter. Manual or external movement between admission and the first setter, or between two ramp steps, stops and holds before another write.

### MDT693B Monitor and Fault Policy

A daemon monitor starts after the initial read-only state snapshot. By default it obtains X/Y/Z actual voltages twice per second under the same request lock. An output operation has priority, so monitor reads cannot interleave with a command/reply or ramp step.

If the monitor observes a voltage above 75 V or above a configured lower ceiling, the driver records the complete observation, blocks further increases, notifies waiters, and enters a latched fault/restricted condition. It does not automatically move the piezo stage.

Three consecutive monitor communication failures, a malformed prompt, a short write, an unconfirmed setter, or a partially failed ramp stop future movement and enter `FAULT`. The last confirmed targets and last observed voltages remain available as evidence. No automatic zero is sent.

`recover()` is explicit. It re-reads the command list, hardware limit, Master Scan, X/Y/Z actual values and limits, and all relevant control state. Recovery succeeds only when communication and current values can be confirmed. Recovery never increases an output.

Recovery may restore communication and restart the monitor, but it never restores axis-command authority. Its status is truthful even when communication is `READY` and `axis_command_known=False`.

### MDT693B Explicit Zero Operations

`ramp_to_zero()` is a state-changing public operation. It coordinates Master Scan and X/Y/Z contributions using the approved slew limit, then disables Master Scan. It does not silently override a user-configured minimum that prevents reaching zero; that condition raises `InstrumentSafetyError` before movement.

`emergency_zero(confirm=True)` cancels active and queued ramps through a monotonic cancellation generation, immediately commands Master Scan voltage to zero, commands all axes to zero, and disables Master Scan. It then queries all three actual outputs.

Neither method claims success unless X/Y/Z actual outputs are confirmed near zero. A manual control or external analog input that keeps an output nonzero causes `DeviceFault` with the measured evidence.

## Concurrency, Cancellation, and Cleanup

Each driver serializes request/reply pairs. A public close transition prevents new work before waiting for the lifecycle lock. Active operations observe cancellation generations or cancellation events at bounded intervals.

MDT693B close stops and joins the monitor before closing the serial resource. Close does not write a voltage or setting. If the monitor cannot terminate or the serial close fails, the driver preserves the resource reference and cleanup evidence rather than claiming `DISCONNECTED` falsely.

PM400 close cancels an active measurement only when needed to release the session, then closes its instrument resource and any owned manager. It never resets measurement settings.

All cleanup steps catch `BaseException` so that `KeyboardInterrupt` and `SystemExit` still trigger resource cleanup. When a context body already has an exception, cleanup errors are logged and retained without replacing the body exception.

## Exceptions

The existing exception taxonomy remains, with one addition:

- `InstrumentConnectionError`: transport open, write, short-write, read, close, or low-level I/O failure.
- `InstrumentProtocolError`: malformed, unexpected, or out-of-order response.
- `InstrumentSafetyError`: unsafe parameter, state, missing confirmation, or voltage transition.
- `InstrumentTimeoutError`: bounded operation or cleanup timeout.
- `InstrumentCapabilityError`: the connected sensor, instrument model, or firmware does not support a documented optional operation.
- `DeviceFault`: current device state or cleanup result cannot be safely confirmed.

## Offline Verification

### PM400 tests

- Connection performs exactly the approved read-only queries.
- Sensor identity and every documented flag combination parse correctly.
- Every documented command has a public typed method or an explicitly documented root operation.
- All measurement kinds configure and parse correctly.
- Capability checks reject unsupported operations before transport writes.
- MIN/MAX/DEFAULT, unit, boolean, enum, date/time, and register boundaries.
- Non-finite inputs and outputs are rejected.
- Setter readback mismatch, stale device errors, malformed SCPI responses, timeouts, cancellation, abort, and response-queue recovery.
- Shared and owned ResourceManager cleanup.
- Duplicate resource-open rejection.
- No thread, VISA session, or manager leak.

### MDT693B tests

- Echo on/off and CR, LF, and CRLF response combinations.
- Single-line/multi-line results and `*`/`!` prompts.
- Connect and default close contain no writes.
- 75 V, hardware-limit, application-limit, and axis-min/max intersections.
- Bool, NaN, infinity, negative, and out-of-range rejection before writes.
- Exact 0.1 V/50 ms coordinated ramp behavior with fake time.
- Master Scan and actual X/Y/Z confirmation.
- Restricted startup above the software limit permits reads and reductions only.
- The 2 Hz monitor detects external/manual over-limit values without issuing zero.
- Short writes, malformed replies, monitor failure, partial ramp, cancellation, fault retention, and explicit recovery.
- Normal zero and emergency-zero command order and confirmation.
- Normal close preserves outputs and terminates the monitor.
- Cleanup failures and `BaseException` preserve primary evidence.
- No monitor-thread or serial-resource leak.

Tests reside in `Code/Debugs/test_pm400.py` and `Code/Debugs/test_mdt693b.py` and run under Anaconda environment `VISA`. Existing driver tests remain green.

## Hardware Diagnostics

`check_pm400.py` proceeds in stages:

1. Enumerate VISA resources.
2. Open only the selected PM400 resource.
3. Read identity and sensor capabilities.
4. Read current measurement configuration.
5. Perform one measurement using the current configuration.
6. Offer separately confirmed, reversible no-op setter checks.
7. Close without reset, zero, or configuration change.

`check_mdt.py` proceeds in stages:

1. Enumerate serial resources and identify the intended USB serial device.
2. Open with the documented serial settings.
3. Query `?`, identity, firmware, hardware limit, X/Y/Z, Master Scan, and all configuration values.
4. Resolve the `xmin?` manual ambiguity from the real firmware without writing.
5. Close while preserving output.
6. Only after separate operator approval, perform one 0.1 V single-axis step, read all three outputs, and explicitly restore the prior value through the normal ramp.

The MDT diagnostic also requires a known axis-command baseline. A fresh read-only diagnostic connection has no such baseline, so it must not disclose a MOVE target, prompt for MOVE, or write an axis. A separately authorized workflow may first establish a baseline through confirmed emergency zero or the explicit operator-attested adoption API; the diagnostic itself must not silently adopt actual output as a base command. If actual XYZ changes while the diagnostic is waiting for confirmation, it aborts before movement or restoration.

No hardware diagnostic runs until the complete offline suite passes. Every state-changing stage states the exact axis, voltage, and restoration behavior before requesting approval.

## Persistent Workspace Rules

`AGENTS.md` will add:

- PM400 connect and close preserve current front-panel measurement configuration.
- PM400 reset, zero, detector response, and adapter-type changes require explicit operator authorization.
- PM400 operations are gated by connected-sensor capabilities.
- Negative PM400 capability flags fail closed as protocol errors.
- MDT693B connect and normal close are read-only and preserve outputs.
- MDT693B software output ceiling is 75 V, with lower per-axis limits allowed.
- Normal MDT693B changes use at most 0.1 V per 50 ms.
- MDT693B faults stop and hold; they do not automatically zero the piezo.
- Only explicit zero operations may move the stage to zero.
- Read-only MDT693B connect/recovery leave absolute motion disarmed until verified emergency zero or explicit operator-attested baseline adoption.
- Experiment code may not bypass either driver with raw VISA or serial communication.

## Acceptance Criteria

- Every documented PM400 SCPI command is represented by a typed public method or documented root operation.
- Every documented MDT693B remote command is represented by a typed public method, subject to real-firmware confirmation of arrow sequences and the `xmin?` ambiguity.
- PM400 sensor capabilities are discovered rather than hard-coded.
- Negative PM400 capability flags fail closed as protocol errors before any setter; unknown nonnegative bits remain preserved.
- PM400 connect/close do not alter measurement settings.
- PM400 and OSA can hold independent VISA sessions without resource-manager ownership conflicts.
- MDT693B connect/close issue no output or configuration writes.
- MDT693B never commands or permits a target above 75 V.
- All ordinary MDT693B voltage changes obey 0.1 V/50 ms.
- Read-only MDT693B connect/recovery never fabricate a base-axis command; absolute motion is zero-write blocked until a verified zero or explicit operator-attested baseline exists.
- Getter/monitor/manual/external deviations invalidate a known baseline before a subsequent write, and multi-step motion revalidates before every setter.
- MDT693B monitor faults do not cause unsolicited movement.
- Explicit zero methods verify actual X/Y/Z results and report external-input interference truthfully.
- All new and existing offline tests pass in `VISA`.
- Staged hardware checks validate read-only connection before any approved output change.
- Debug scripts and `AGENTS.md` preserve these rules for future experiment work.

## Operational Limits

Software cannot guarantee cleanup after host power loss, forced process termination, USB disconnection that prevents communication, controller power loss, firmware failure, or an external/manual signal that changes output after the last successful query. The MDT693B rear-panel limit, piezo rating, physical grounding, and operator control remain the final safety layers.
