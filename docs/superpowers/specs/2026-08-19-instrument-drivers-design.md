# Instrument Driver Design

## Purpose

Build a small, reusable, and fail-safe Python driver layer for three laboratory instruments:

- Yokogawa AQ6370 optical spectrum analyzer over VISA/GPIB.
- Custom eight-channel voltage source over a CH340 serial interface.
- Custom Gain Chip Driver over a CP210x serial interface.

The existing `reference_code` directory remains unchanged and is used only as protocol reference. New experiments must use the driver layer rather than communicate with serial or VISA devices directly.

## Project Layout

```text
Code/
├── Utils/
│   ├── __init__.py
│   ├── common.py
│   ├── osa.py
│   ├── voltage.py
│   └── gain.py
├── Debugs/
│   ├── test_drivers.py
│   ├── check_osa.py
│   ├── check_voltage.py
│   ├── check_gain.py
│   └── check_all.py
└── Experiments/
    └── <name>/
Result/
└── <name>/
reference_code/
```

A root-level `AGENTS.md` will preserve the directory, naming, and hardware-safety rules for future AI work in this workspace.

## Architecture

Use three independent drivers with a small shared utility module. Avoid a deep common instrument base class because VISA request/response, continuous serial telemetry, and serial request/response have materially different lifecycles.

Each driver:

- Supports `connect()`, `close()`, and Python context-manager use.
- Makes `connect()` and `close()` idempotent.
- Owns its connection, locks, state, timeouts, and cleanup.
- Validates state and parameters before every public operation.
- Uses structured return values rather than exposing raw bytes or strings.
- Records connection, state transition, fault, and shutdown events through Python logging.

The common module defines shared state values and concise exceptions such as connection, protocol, safety, timeout, and device-fault errors.

## Driver State Model

```text
DISCONNECTED -> CONNECTING -> READY -> ACTIVE -> CLOSING
                                |
                                v
                              FAULT
```

Public operations reject invalid states. A fault does not automatically re-enable an output. Recovery requires an explicit close and reconnect, or a dedicated reset operation where safe.

Cleanup continues even if one cleanup step fails. A cleanup failure is logged and attached to the operation outcome without replacing the original experiment exception.

## AQ6370 Driver

### Connection

- Default resource: `GPIB0::4::INSTR`; callers may override it.
- Connect through PyVISA and verify communication and instrument identity.
- Do not modify center wavelength, span, resolution bandwidth, sample count, sweep mode, or other panel settings during connection.

### Acquisition

- Trigger a sweep using the AQ6370 immediate-initiate command.
- Wait for completion with a bounded timeout rather than a fixed sleep where the instrument supports it.
- Read Trace A x-data and y-data by default; allow another trace to be selected explicitly.
- Convert wavelength from meters to nanometers and retain power in dBm.
- Return wavelength, power, trace name, acquisition time, and instrument identity as structured data.
- Validate non-empty arrays, matching lengths, finite values, and plausible ordering before returning data.

### Failure Handling

- Default acquisition timeout: 30 seconds, overridable per call.
- On timeout or acquisition failure, send `ABORT` and retry up to two times.
- Never return cached or old trace data as if the failed acquisition succeeded.
- After final failure, abort any active operation, close the VISA resource, and raise an explicit acquisition error.
- Closing aborts an unfinished sweep before releasing VISA resources.

## Voltage Source Driver

### Discovery and Connection

- Auto-discover a CH340 device by VID:PID `1A86:7523`; allow an explicit COM port override.
- If discovery finds zero or multiple matches, fail clearly instead of guessing.
- Serial configuration: 115200 baud, 8 data bits, no parity, one stop bit, bounded read/write timeouts.
- Start the telemetry reader, obtain a valid complete monitoring frame, send an eight-channel zero command, and confirm all measured voltages are below 0.05 V before entering `READY`.

### Output Protocol

- Every command contains all eight channel setpoints.
- Encode each voltage as unsigned 16-bit big-endian: `int(voltage / 28 * 65535)`.
- Append CRLF, producing an 18-byte command frame.
- Enforce a project hard limit of 0 to 14 V even though the board protocol supports a higher voltage.
- Reject NaN, infinity, negative values, incorrect channel counts, and values above 14 V before writing anything.

### Normal Control

- Provide `set_channel()`, `set_all()`, `ramp_to()`, `read_status()`, and `zero()`.
- Normal voltage changes ramp in steps no larger than 0.1 V with 50 ms between steps.
- A lock ensures only one ramp or write sequence can control the output at a time.
- An emergency zero bypasses the ramp and sends all zeros immediately.

### Monitoring Protocol

- Continuously read CRLF-terminated frames in a background thread.
- Require an expected frame of 32 payload bytes plus CRLF.
- Parse eight channel records, each containing unsigned big-endian voltage and signed two's-complement big-endian current.
- Convert voltage as `raw * 0.0016 V` and current as `raw * 0.0025 mA`.
- Publish an immutable status snapshot containing eight voltages, eight currents, and a timestamp.
- Reject, log, and resynchronize after malformed frames rather than indexing partial data.

### Failure Handling

- Detect stale telemetry, repeated malformed frames, write timeouts, reader-thread death, and unexpected disconnects.
- On a monitoring or communication fault, attempt an immediate eight-channel zero command and enter `FAULT`.
- `close()` attempts immediate zero, stops and joins the reader thread with a timeout, then closes the serial port.
- Repeated calls to `zero()` or `close()` remain safe.

## Gain Chip Driver

### Discovery and Connection

- Auto-discover the CP210x interface by VID:PID `10C4:EA60`; prefer its stable USB serial number when available and permit an explicit COM port override.
- Serial configuration: 115200 baud, 8 data bits, no parity, one stop bit, bounded read/write timeouts.
- All commands and responses are ASCII and use CRLF framing. The hardware debug check will confirm the response terminator before output-changing tests.
- Serialize all requests under one lock so the watchdog and foreground operations cannot interleave commands or responses.

### Command and Response Protocol

- Support temperature, target temperature, TEC state, PID coefficient, current setting, and current-output state read commands documented in the reference command file.
- Support the corresponding fixed-width write commands plus reset and integral-clear commands.
- Parse responses in the form `READY;<field>=<value>` and validate that the response field matches the request.
- Treat missing prefixes, unexpected fields, invalid numeric values, extra data, and timeout as protocol errors.

### Safety Limits and Interlocks

- Target temperature hard range: 15 to 40 degrees Celsius.
- Drive-current hard range: 0 to 200 mA.
- Before enabling current output, require:
  - An active connection without a fault.
  - TEC reported as enabled.
  - Actual temperature within target plus or minus 0.2 degrees Celsius for five consecutive seconds.
  - A valid current setpoint within the hard limit.
- These checks are internal to `enable_current()` and cannot be bypassed by experiment code.

### Temperature Watchdog

- Poll actual temperature once per second while connected.
- If temperature remains more than 1 degree Celsius from target for three consecutive seconds, immediately disable current output while keeping TEC enabled for recovery.
- If temperature differs by more than 3 degrees Celsius, or three consecutive temperature reads fail, disable current, then disable TEC, and enter a latched `FAULT` state.
- A fault never automatically re-enables TEC or current.

### Shutdown

- Normal close, context-manager exit, handled exception, and `KeyboardInterrupt` execute the same order: disable current, disable TEC, stop the watchdog, and close the serial port.
- Every shutdown step is attempted even if an earlier step fails.
- Shutdown operations are idempotent and preserve the original exception.

## Debug and Verification Programs

### Offline Tests

`Code/Debugs/test_drivers.py` uses fake serial and VISA transports. It verifies:

- Command encoding and response parsing.
- Voltage telemetry framing and conversion.
- All hard boundaries and invalid numeric inputs.
- Retry and timeout behavior.
- Interlock and watchdog transitions.
- Cleanup ordering and idempotence.
- Failures during partial connection, acquisition, ramping, watchdog polling, and shutdown.
- Thread termination and resource release.

The tests use the standard library where practical to avoid unnecessary dependencies.

### Hardware Checks

- `check_osa.py`: verify identity, acquire one spectrum without changing panel settings, and validate returned arrays.
- `check_voltage.py`: verify discovery and telemetry, perform startup zero and confirmation, then offer an explicitly confirmed CH1 0.1 V test that always returns to zero.
- `check_gain.py`: verify discovery, response framing, all read commands, and no-op writes of current values; output-enabling tests require explicit interactive confirmation.
- `check_all.py`: run the three checks in a safe order and require confirmation before any nonzero output.

Debug programs do not contain experiment logic or save results under experiment result directories.

## Workspace Rules

The root `AGENTS.md` will require future work to follow these rules:

- Reusable instrument drivers belong in `Code/Utils`.
- Hardware bring-up and diagnostic programs belong in `Code/Debugs`.
- Each experiment belongs in `Code/Experiments/<name>`.
- Experiment output belongs in `Result/<name>`.
- `reference_code` is read-only reference material.
- Experiment code must use the drivers rather than access serial or VISA devices directly.
- Names must remain short and descriptive.
- Hardware limits, startup checks, interlocks, and fail-safe shutdown behavior cannot be bypassed.
- Read-only checks precede state-changing hardware diagnostics, and state changes must be made explicit to the user.
- Generated data, caches, and temporary files do not belong in `Code/Utils`.

## Operational Limits

The software handles normal return, Python exceptions, context-manager exit, and `KeyboardInterrupt`. It cannot execute cleanup after computer power loss, forced process termination, USB disconnection that prevents writes, or a hardware/firmware failure. Hardware-side voltage/current limits and watchdogs remain the final safety layer.

## Acceptance Criteria

- The driver modules expose the approved lifecycle and control interfaces.
- All offline tests pass in the Anaconda `VISA` environment.
- Hardware checks confirm connection, telemetry or response parsing, safe control, timeout behavior, and cleanup for all three instruments.
- Voltage is zeroed on startup, normal close, and handled failure.
- Gain current and TEC obey all limits, stabilization requirements, watchdog trips, and shutdown ordering.
- OSA acquisition preserves panel settings and never reports a timed-out acquisition as successful.
- The workspace structure and `AGENTS.md` enforce the approved organization for future AI work.
