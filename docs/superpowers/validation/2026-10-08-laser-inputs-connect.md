# Native connection admission and laser controls — 2026-10-08

Baseline: `e0a5a98921afffd0b551bc9e1602ea01c091726a`.

Gain/Voltage normal connection already carried Host-bound authorization, but the Host writer's older connect validator rejected it before Worker delivery with `Invalid v3 method parameters/context`. Connect now uses the shared strict protocol parser. A regression constructs real Host authorization, passes it through the production writer and checks the Worker wire request. Malformed/unconfirmed authorization remains rejected; leases, context and driver lifecycle checks are preserved.

Laser scan drafts initialize inside the reviewed operating range. Start uses the rounded and bounded setpoint, or the lower bound if unavailable. Stop uses the upper bound unless it is less than the native 0.01 nm minimum span away. Near the upper bound, it uses the lower endpoint; a narrow midpoint range uses both endpoints. A range too small to scan leaves start scanning unavailable with an explanation. These are editable proposed settings, not scanner register readbacks; initializing them sends no command.

The first click selects the digit under the pointer in Target, both scan endpoints and both velocity inputs, including the right half of each digit. Selection uses rendered monospace geometry rather than the browser caret or the focus default. Keyboard digit stepping and telemetry draft preservation remain intact.

Control gives scanning more horizontal space. Target is larger; titles align at the top, output/Tracking and scanning actions align at the bottom, and collapsed options follow the same widths. Narrower windows stack the control groups.

## Verification

- RED reproduced the exact Host rejection, empty Stop, first-click selection `[7,8]` instead of `[0,1]`, and too-short near-upper default scan.
- **547 Rust tests**, **388 Node tests**, and **13 native packaging groups** passed.
- Production modules in a finite injected Edge renderer passed every digit position, initialized endpoints, draft preservation, target coalescing, scan/stop and saved-limits/reconnect. Layout passed at 800, 960 and 1440 px, including desktop action-row alignment.
- Scoped independent review identified the minimum-span edge; the fix passed follow-up review with no remaining findings.

Evidence is under `Result/laser-input`. No real-device connection, output diagnostic or driver installation was performed by implementation/qualification tools. An authenticated read-only check found the existing Host still owns one READY instrument; that owner has not been interrupted.

## Qualified portable candidate

`Result/native-package/laser-controls-20261008-02/portable` contains 29 approved files: three native executables and 26 resources, including three original driver packages and 18 pinned vendor files. No Python payload.

- Source: `tree-5d3f3cdaaabf7ae54e31478423710c7cde6ddfe100a230091340f4fb9b93da57`.
- Package: `0.1.0-5d3f3cdaaabf`.
- GUI SHA256: `1caffec224ee160e61ba6b429276f3f8e91111f7f61791f4122a49e4f75d992b`.
- Host SHA256: `2302c8633271dfb42450df315f62feeb81b49a64fe3dc85261cb5d3713cd074d`.
- Worker SHA256: `a6feac51c5995c3c40ddd34817e719b3fccbe524cf26951f7f148a59ac6861e8`.
- ZIP: 23,306,460 bytes; SHA256 `29eaf9e5e641f04ded69f7658923696fe8c2a78a84c4b40bf10daa2ef5d14e3f`.

Actual PE inspection confirmed AMD64 and no Python or dynamic MSVC CRT imports. The exact packaged Worker returned disarmed/empty identity on a System32-only PATH and exited normally on EOF with code 0. The candidate has not replaced the live Host. Installer, clean-Windows and physical qualification remain unclaimed.

The later four-action design is Full (Start→Stop→Start), Forward (current→Stop, halt), Backward (current→Start, halt), and Stop (hold). This candidate retains the existing full cycle and hold-position Stop. The [manufacturer manual, pp. 63–64 and 68–72](https://manuals.plus/m/3e7b05f35b8870a25f91b866c2c69306213ea52c1ae81ed6ca07fdc5dcb38076.pdf) documents RESET movement to Start but does not explicitly bind its speed to SLEW:RET or state its blanking behavior. Speed-controlled one-way actions require confirmation before claiming that behavior; no timed UI Stop or unqualified RESET substitution was introduced.
