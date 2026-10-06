# Tauri instrument console design

## Intent and scope

Build a Windows desktop console for the instruments already represented by this repository: Yokogawa AQ6370 series OSA, the custom eight-channel Voltage Source, the custom Gain Chip Driver, PM400, and the left and right MDT693B-controlled NanoMax fiber stages. The operator chooses which discovered devices belong to a session, sees live status and device-specific controls, and can understand both fiber stages in one laboratory coordinate system. This app is a manual control surface, not an experiment sequencer.

The first launch must never connect to an instrument or change an output. Enumeration is an explicit refresh action. Connections and actions are separately initiated by the operator. A session can contain any subset; a missing stage side is valid. A device that was visible at enumeration may disappear before connection, so every connection validates identity again through its driver.

## Architecture

`App/` contains a Tauri 2 desktop shell and a static HTML/CSS/JavaScript frontend. This avoids a Node package manager on the bench PC; Tauri serves the static files in development and bundles them in production. A single persistent Python worker launched from the installed Anaconda `VISA` interpreter owns all drivers. Rust owns the worker process, serializes JSON requests over standard input, correlates responses by ID, and reports worker exit and stderr to the UI. The worker runs from a known project root, imports only `Code/Utils` and `Code/Setups` for instrument operations, and offers an explicit allowlist of methods. It never offers raw SCPI, raw serial writes, or arbitrary Python execution. No network listener is opened.

One worker owns all devices so Python's process-local VISA reservations apply across OSA and PM400. The Rust process ends the worker gracefully on app close; the worker attempts driver-specific cleanup, preserves errors in a cleanup report, and never auto-zeros MDT outputs. The app distinguishes requested settings, observed telemetry, and stage position estimates. A worker crash or lost IPC marks status unknown; the UI cannot report a successful shutdown without a response.

The Python path and project root are app settings. The initial default Python path is the current machine's `D:/SoftwareInstaller/Anaconda/envs/VISA/python.exe`. The app validates the path before launching and checks that the child reports its Python executable and imported project root. The backend protocol has a version field and strict request schemas. Errors include a safe message, type, and the most recent known device state.

## Discovery and configuration

Refreshing the inventory lists serial metadata and VISA resource names without opening devices. The UI groups recognizable serial adapters by USB metadata and displays unassigned ports/resources separately. The operator can bind OSA and PM400 to VISA resources and the custom boards to COM ports. Fiber side is fixed by USB serial `2110148249-10` (left) and `160721175410` (right), never by COM order. The app stores role selections and display preferences locally, but never persists armed state, adopted baseline, estimated position, or an assumption that a device remains connected.

The configuration screen shows the effect of each connection: OSA close can abort a sweep; Voltage Source connect commands all eight channels to zero; Gain connect can shut down an invalid current/TEC combination; PM400 and Fiber Setup connect are read-only. A changing-output diagnostic or connect path receives a distinct confirmation in the UI. Resource collisions are rejected before connection and again by the drivers.

## Panels

- **Overview:** selected/discovered/connected/error state for every role, last status time, and a clear aggregate cleanup report. An unavailable device remains visible with its last known identity and a stale badge.
- **OSA:** identity, connection and acquisition state, trace selector, one-shot acquire, wavelength/power plot, cursor readout, and CSV export of the acquired trace. Front-panel measurement settings remain under panel control. No sweep starts on launch.
- **Voltage Source:** eight independent cards with requested voltage, observed voltage and current, a compact trend, an explicit apply control, and an all-channel zero control. Show telemetry age and distinguish command sent from host-observed zero evidence. Driver enforces 0–14 V and its ramp policy.
- **Gain Driver:** measured and target temperature, current setpoint and enabled state, TEC and current controls, watchdog/fault state, and a temperature trend. Enable current stays disabled in the UI until TEC is on, but the driver remains the authority for its five-second stability interlock. Shutdown order is current then TEC.
- **PM400:** sensor identity/capabilities, selected measurement kind and unit, numeric reading and trend, with organized measurement, sensor configuration, status/system, and advanced tabs. Each advanced operation maps to a typed PM400 driver method. Reset, zero/calibration, detector-response, and adapter changes require explicit confirmation and show the affected sensor/parameter. Unsupported controls are disabled based on sensor capabilities.
- **Fiber stages:** both sides shown together. Each shows logical X right, Y away from the operator, Z up, three observed voltages, connection/restriction/fault state, baseline authority and nominal-calibration status. An interactive 3D-like stage model can rotate; labeled arrows and a fixed lab-coordinate compass make the commanded directions unambiguous. Relative movement is entered in micrometers with a preview of the signed displacement, toward-chip classification and maximum allowed step. The UI displays estimated displacement only after explicit baseline adoption and labels it session-only open-loop. Baseline adoption requires operator attestation and a separate nominal-MAX312D acknowledgment. Fault or manual movement stops and holds; the estimate becomes unknown. No automatic stage zero or rollback.

## Safety and concurrency

All driver limits in `AGENTS.md` remain binding. The UI validates inputs for immediate feedback, and the Python adapter calls public driver/setup methods so a stale or malformed UI cannot bypass limits. One mutating action per device runs at a time; status polling never consumes responses needed by the action. Long operations report busy state and allow only supported cancellation/close semantics. Loss of window focus is not a stop command. Closing the app requests orderly driver close and displays any incomplete cleanup; the host process does not silently claim physical zero or a released port.

Device discovery and read-only connection are separate stages. A reversible write to actual hardware remains a separate operator-approved action. The application can be developed and exercised in simulation without touching connected instruments. Starting the app does not itself imply authorization for real hardware actions.

## Verification and acceptance

Use Anaconda `VISA` for every Python command. Tests cover protocol validation, inventory role binding, disconnection and worker-crash truthfulness, serialized ownership, per-device adapters with fake drivers, cleanup ordering, and offline simulated UI flows. Run Node's built-in JavaScript test runner and build the Rust shell. On this Windows host, a real Tauri build needs Rust MSVC, C++ Build Tools, and WebView2; the static frontend needs no npm installation. Missing prerequisites are recorded rather than being disguised by a web-only preview. Hardware checks use `Code/Debugs` and the staged approvals in `AGENTS.md`. The September 11 MDT `?` query timeout remains an unresolved hardware-compatibility issue until separately diagnosed.
