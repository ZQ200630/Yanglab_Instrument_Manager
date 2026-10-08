# Desktop startup and simple connection — 2026-10-08

Baseline: merged main `6871e1e0de73cce4756650dec4a07ad8e9547e25`.

The app starts and attaches its native Host automatically. An incompatible existing Host is explained persistently; startup progress remains visible across navigation. An unconfirmed launch permits attachment checks only, including after transient IPC failures, until a valid connection clears the fence.

Normal Local Host settings show status, with one retry action when unavailable. Host name, measurement data folder, and stop/disconnect management remain inside closed Advanced settings. Folder selection saves immediately; applying the folder still requires a safe Host restart and never interrupts instruments automatically.

Serial discovery uses metadata-only Windows enumeration. Gain suggests both CH340/CH341 and CP210x ports; multiple candidates require selection. Driver readiness joins the selected port's full Windows instance ID to exactly one device record, ignoring casing. An unrelated adapter's missing driver cannot block a healthy selection. Ambiguous or unmatched IDs remain unconfirmed. Removal or instance replacement invalidates proof, including manual port mode. A detected missing CH340 can be installed before Windows assigns a COM port; installation refreshes the new port. Manual entry, all ports, baud rate and timeout remain available under Advanced connection settings.

Gain and Voltage use one explicit **Connect & verify** action. The GUI establishes the acknowledged normal connection before requesting its existing supervised identity proof. It does not bypass controller leases, configuration binding, identity verification, failure retention, or unknown-outcome fences. Unknown initialization never proceeds to verification/save. Ordinary Gain connection now acknowledges current-off before TEC-off, reads all five status fields, and requires both outputs disabled before Ready. Target, PID and current setpoint remain intact. Read-only probes preserve existing outputs. Voltage retains its startup/shutdown zero; MDT piezo retains hold behavior.

Saved Laser connections now receive the existing bound read-only identity check before native connection. Saved records alone do not create driver authority.

## Verification

- Full offline native runner: 545 Rust tests and all 13 finite package script groups passed, exit 0. No Python on its test PATH.
- Final frontend after settings simplification: 387 Node tests passed, exit 0.
- Gain/catalog metadata covering run: 43 tests passed after the final catalog change.
- Scoped review covering four frontend suites: 97 passed; findings closed.
- Edge renderer fixture: collapsed management, one offline retry, compatible port choices, default initialization controls, and 800/960/1440 px layouts passed. Primary connection action is visible without scrolling. Fixtures have no hardware access.

Evidence is retained under `Result/startup-serial`, including RED/GREEN logs, final native/frontend logs, browser screenshots and review receipts. No new real-device connection, output diagnostic or driver installation was performed by the implementation/qualification tools.

## Qualified and launched portable package

`Result/native-package/simple-connect-20261008-01/portable` was built with locked offline Cargo and existing MSVC tools. Its 29-file contract contains three native executables and 26 resources, including all three original vendor packages with 18 pinned vendor files. It contains no Python payload.

- Source identity: `tree-e838791489e00162f88a9951d675f70967167967b3406db3f98febf3c742ae7e`.
- Package revision: `0.1.0-e838791489e0`.
- GUI SHA256: `70614bc9e31d6acd8c9b4beb88e200327dd3ad4d140028ef0de12dfe48567f50`.
- Host SHA256: `6bc9191764dfc285388e91054dd48fbe82d8e62b0206cdbe42392af910ee8eeb`.
- Worker SHA256: `2ea9fdcd4d42875c5c8e608a5b00ed6803d3e353919516e2763ae411e0c85e19`.
- ZIP size: 23,263,639 bytes; SHA256 `ddd210ea6b25913c81d0e59e506482d76be0b7e88450a842bba906465b5b4925`.

Actual PE inspection confirms AMD64 and no Python or dynamic MSVC CRT imports. The exact packaged Worker returned disarmed/empty native identity on a System32-only PATH and confirmed clean EOF exit 0.

After the operator normally closed the previous GUI, authenticated read-only Host checks showed disconnected domains with no physical resource responsibility or pending work. The formal Host stop confirmed resource release and successful Worker exit separately; normal Host exit was also confirmed. No process was force-killed. The replacement GUI was then launched: its own packaged Host and Worker appeared with the expected parent chain, and authenticated IPC confirmed Rust/protocol 3, verified startup and confirmed activation. Instrument domains remained disconnected. Activation is not physical output or zero evidence.

NSIS installer, clean-Windows deployment and physical instrument qualification remain unclaimed. The Windows Computer Use helper failed initialization; actual native startup was verified through process identity and authenticated IPC, while visual evidence comes from the bounded browser renderer.
