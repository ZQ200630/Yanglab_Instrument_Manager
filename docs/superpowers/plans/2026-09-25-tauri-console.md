# Tauri Instrument Console Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a Windows Tauri 2 control console for every instrument family in `Code/Utils`, with configurable discovery, distinct device panels, and a paired 3D fiber-stage view.

**Architecture:** A single persistent Python worker in Anaconda `VISA` owns all existing drivers and communicates with a Tauri Rust host over JSON lines. The frontend calls typed Tauri commands and never opens instrument transports. A simulated worker makes every panel testable offline.

**Tech Stack:** Python 3.10 in conda `VISA`, Rust MSVC, Tauri 2, static HTML/CSS/JavaScript, Node's built-in test runner, CSS 3D.

**Spec:** `docs/superpowers/specs/2026-09-25-tauri-console-design.md`

## Global Constraints

- `AGENTS.md` is binding; every Python invocation uses Anaconda `VISA`.
- No device connection or output action occurs merely from starting the app.
- Fiber side binding is by USB serial `2110148249-10`/`160721175410`; UI coordinates are +X right, +Y away, +Z up.
- Voltage 0–14 V, Gain 0–200 mA and 15–40 °C, MDT 75 V and stop-and-hold are enforced by existing public drivers.
- Real hardware diagnostics follow separate enumeration, read-only connection, and reversible-action authorization stages.
- Use `App/` for console files; keep `Code/Utils` unchanged unless a proven adapter need requires a separately tested driver fix.

## Review Focus

- A device disappears between enumeration and connection: show an explicit error and no connected badge (Task 2 test).
- Two roles choose the same canonical VISA resource: refuse the second connection (Task 2 test).
- A worker dies during an output-changing call: show outcome unknown, never successful cleanup (Task 3 test).
- A stage has residual voltage but no adopted baseline: show voltage while position/move authority remains unknown (Task 5 test).
- A PM400 sensor lacks a capability: disable the operation before any write and surface the driver reason if invoked anyway (Task 6 test).

---

### Task 1: Worker protocol, discovery, and saved configuration

**Files:** Create `App/worker/{__init__,protocol,discovery,settings}.py`, `App/tests/test_worker_foundation.py`.

**Interfaces:** `parse_request(line: str) -> Request`; `encode_response(request_id: str, *, result=None, error=None) -> str`; `discover() -> dict`; `load_settings(path: Path) -> dict`; `save_settings(path: Path, settings: dict) -> None`.

- [ ] Write tests for invalid/duplicate fields, disconnected startup, serial metadata classification and atomic settings save. Example: `self.assertEqual(parse_request('{"id":"a","method":"ping","params":{}}').method, "ping")`.
- [ ] Run `conda run -n VISA --no-capture-output python -B -m unittest App.tests.test_worker_foundation -v`; expect failure because modules do not exist.
- [ ] Implement strict JSON schema, read-only enumeration through `serial.tools.list_ports` and `pyvisa.ResourceManager.list_resources()`, then atomic local settings writes. Discovery must close the manager in `finally` and report partial errors without inventing identities.
- [ ] Rerun the same test module; expect all tests passing.
- [ ] Commit only Task 1 files with `git add App/worker App/tests/test_worker_foundation.py` and `git commit -m "feat: add console worker protocol and discovery"`.

### Task 2: Session owner and typed device adapters

**Files:** Create `App/worker/{controller,adapters,simulation}.py`, `App/tests/test_worker_controller.py`.

**Interfaces:** `ConsoleController(*, simulate: bool, factories: Mapping[str, Callable] | None = None)`; `handle(method: str, params: dict) -> dict`; `close() -> dict`. Methods: `inventory`, `connect`, `disconnect`, `status`, `action`. Device roles: `osa`, `voltage`, `gain`, `pm400`, `fiber`.

- [ ] Write fake-driver tests for every role: read-only connect where appropriate, Voltage startup hazard declaration, per-role identity/status, duplicate VISA resource rejection, missing side, stage baseline unknown, and cleanup order.
- [ ] Run `conda run -n VISA --no-capture-output python -B -m unittest App.tests.test_worker_controller -v`; expect missing controller/adapters.
- [ ] Implement allowlisted actions by calling public `AQ6370`, `VoltageSource`, `GainDriver`, `PM400`, `FiberCouplingSetup` APIs. Make simulation use matching result shapes with no hardware transport. Never pass arbitrary method names from requests to `getattr` on a driver.
- [ ] Rerun controller and foundation tests; expect pass and no newly live worker threads.
- [ ] Commit Task 2 files.

### Task 3: Persistent worker process and lifecycle

**Files:** Create `App/worker/main.py`, `App/tests/test_worker_process.py`.

**Interfaces:** `python -B -m App.worker.main --simulate` accepts one JSON request per line and writes one JSON response per line. `--real` enables explicit live actions; launch alone remains disconnected.

- [ ] Write subprocess tests for ping/inventory/simulated connect/action/disconnect, malformed JSON, EOF cleanup, and a forced child exit during a pending call.
- [ ] Run `conda run -n VISA --no-capture-output python -B -m unittest App.tests.test_worker_process -v`; expect failure before implementation.
- [ ] Implement process loop, request IDs, version handshake, stderr logging and final `controller.close()` evidence. Keep stdout exclusively for protocol frames.
- [ ] Rerun worker suite, then all `App.tests`; expect pass.
- [ ] Commit Task 3 files.

### Task 4: Tauri host and connection settings

**Files:** Create `App/web/{index.html,style.css,main.js,api.js}`, `App/src-tauri/{Cargo.toml,build.rs,tauri.conf.json}`, `App/src-tauri/src/{main,worker}.rs`, `App/tests/host.test.mjs`.

**Interfaces:** Tauri commands `worker_start(config)`, `worker_request(request)`, `worker_stop()`; frontend `request(method, params): Promise<Reply>` through the Tauri global API.

- [ ] Write Rust request/response correlation tests and frontend API shape checks; include process exit, invalid Python path and stale response ID cases.
- [ ] Run `cargo test --manifest-path App/src-tauri/Cargo.toml` and `node --test App/tests/host.test.mjs`; expect missing project files.
- [ ] Create Tauri 2 shell, persistent child pipe owner, path validation, event/status reporting, and local settings view. In dev, resolve project root; in installed mode use bundled resources.
- [ ] Run Rust tests, `node --test App/tests/*.test.mjs`, and `cargo tauri build` on Windows; expected output is a launchable binary and zero test failures. Record prerequisite errors exactly if host lacks Rust/C++/WebView2.
- [ ] Commit Task 4 files.

### Task 5: Overview, OSA, Voltage, Gain, and paired Fiber UI

**Files:** Create `App/web/{overview,osa,voltage,gain,fiber,stage3d}.js`, extend `App/web/style.css`, `App/tests/ui.test.mjs`.

**Interfaces:** Each panel consumes normalized `status` payloads and issues typed `action` requests through `api.js`. `stage3d.js` takes side and logical displacement estimate; it never creates motion commands itself.

- [ ] Add UI tests for idle startup, stale telemetry, Voltage zero evidence labels, Gain interlock disabled state, both fixed fiber identities, residual voltage with unknown stage position, and signed direction preview.
- [ ] Run `node --test App/tests/ui.test.mjs`; expect failures for missing panels.
- [ ] Implement device-specific panels and a rotateable CSS 3D stage scene with visible lab axes and toward-chip arrows. Wire controls to driver adapters and require confirmation text for state-changing actions.
- [ ] Run `node --test App/tests/*.test.mjs`, `node --check` for the frontend modules, and a visual smoke test of the Tauri window; expect pass and no real device I/O on launch.
- [ ] Commit Task 5 files.

### Task 6: PM400 full typed panel and advanced actions

**Files:** Create `App/web/pm400.js`, extend `App/worker/controller.py`, `App/tests/test_worker_pm400.py`, `App/tests/ui.test.mjs`.

**Interfaces:** PM400 reads expose identity, sensor capabilities and measurements; advanced operations are a finite registry mapping UI controls to documented `PM400` facade calls, never raw SCPI.

- [ ] Add tests for every UI-exposed advanced operation, sensor capability rejection before write, confirmation-gated reset/zero/response/adapter changes, and error propagation.
- [ ] Run the PM400 tests before implementation; expect missing actions/panel.
- [ ] Implement measurement, sensor, system/status and advanced tabs, using the driver's typed facades and current capability snapshot.
- [ ] Run Python and UI tests; expect pass. Check that every public PM400 capability needed by the operator has a UI path or an explicitly documented unsupported reason.
- [ ] Commit Task 6 files.

### Task 7: Packaging, acceptance, and hardware handoff

**Files:** Modify `App/README.md`, `README.md`, `docs/acceptance.md`; add packaging assets under `App/src-tauri/icons` and offline acceptance evidence under `Result/console`.

**Interfaces:** `cargo tauri build` yields a Windows installable artifact; packaged worker resolves the Anaconda VISA interpreter and bundled repository code.

- [ ] Add integration tests for installed-path resolution, local configuration migration, app close cleanup, and process crash reporting.
- [ ] Run full Python offline suite in `VISA`, Rust tests, frontend tests/syntax checks, then Windows Tauri build; record versions and exact results.
- [ ] Manually inspect every panel in simulation and verify orientation, labels, keyboard behavior, scroll and window resizing.
- [ ] Record live hardware validation separately under `Code/Debugs` staged permissions. Do not mark MDT motion or PM400 measurement verified from simulation.
- [ ] Commit documentation and packaging changes, review full branch, and integrate as authorized.
