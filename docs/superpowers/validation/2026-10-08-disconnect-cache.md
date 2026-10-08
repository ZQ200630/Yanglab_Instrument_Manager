# Completed disconnect with retained readings

## Observed failure and cause

The operator reported being unable to disconnect and requested software
initialization. Authenticated ping, Host snapshot and Worker scheduler metadata
completed in 31 ms. Both status paths showed DISCONNECTED, no connection ID,
no resource responsibility, zero pending work and a completed preserving-close
receipt with no unreleased resources. The cached last Laser sample still said
connected:true. The native window responded; this does not establish the
renderer state by itself.

The UI release predicate required an empty device sample. It therefore rejected
this real, completed release and could leave Disconnecting/Release unconfirmed
visible. Its view projection also treated the old sample as an active device;
the browser regression exposed old scan defaults being carried across a limits
change and reconnect.

## Change

Cached readings permit reconnect only when the current synced Host reports
AVAILABLE control, DISCONNECTED domain, null connection ID, no responsibility
or pending requests, and a completed successful disconnect receipt bound to
the exact current context. Attempt IDs must match, cleanup must have nonempty
successful steps and no unreleased resources. A receipt alone, retained owner,
pending work, failed cleanup or stale context cannot establish release.

The view does not project a DISCONNECTED/null-connection domain's cached sample
into active controls. The original immutable sample remains in scheduler/store
metadata and Diagnostics. Reconnection initializes controls from the current
device sample. No driver, native lifecycle, output limit or lease guard changed.

## Verification

- Three initial RED regressions reproduced the release predicate and mounted
  reconnect failure before repair; a separate RED test reproduced projection.
- **411 frontend tests passed**, including actual mounted disconnect/reconnect,
  post-acceptance snapshot ordering, retained/pending/failed/stale receipts and
  preservation of the cached sample.
- Offline replay of the captured metadata now presents an enabled Connect.
- Production modules in isolated Edge passed the full Laser regression using
  a finite transport that retains last readings on close. Saved limits,
  bounded scan defaults, refreshed reconnect context, keyboard shortcuts and
  responsive layouts remain covered. No hardware transport was used.
- Rust sources were unchanged; the previously recorded 554 native tests remain
  the native baseline. The rebuilt executables require package/startup
  qualification below rather than a new claim of physical instrument testing.

Evidence is under Result/app-freeze. Windows Computer Use initialization exited
twice before exposing a window; the operator then normally exited the App.
No force termination, instrument reset, connection, setter or driver install
was performed by the diagnostics. Software restart evidence is recorded below.

## Qualified package and requested initialization

The corrected portable build is
Result/native-package/disconnect-fix-20261008-01/portable, package revision
`0.1.0-9d39a0f4e0af`, source
`tree-9d39a0f4e0afe5d7a33899b240f1552376f37d39a3808593016f0301999b8c32`.
Package verification confirmed 29 approved files, three bundled driver packages,
18 vendor pins, no Python payload and no test fixtures. PE inspection confirmed
AMD64 and no Python or dynamic MSVC CRT imports. The exact new Worker returned
empty/disarmed native protocol 3 identity on a System32-only PATH and exited
normally on EOF with code 0.

SHA-256:

- GUI: `742684beb8cc07a2fd892e19023e909a09f57b6351d5dc0bf71abf0bce2f55c4`
- Host: `014d51399f9b237b2aebd7dec6cbe69dc9f95296bea1001a38abe82bc0579bfe`
- Worker: `2199937debdc97ffaffde2291e087b1a63b0f4b48246facf835cf3d6268ac399`
- ZIP (23,311,083 bytes):
  `a493805ee842037698cfbd97a35aefcb4b82a776e485dcc70457d0280031a119`

After normal operator GUI exit, the old Host and Worker had also exited before
the management stop attempt; it returned Local Host is not running. No stale
shutdown receipt was reused and their exit codes are unknown. Prior authenticated
metadata confirmed resource release. All old native processes were verified
absent before launching the replacement; none was force-terminated.

Corrected GUI PID 15204 automatically started packaged Host PID 52180 and Worker
PID 18064, with verified paths and parent chain. The App window responded and
had the expected title and nonzero handle. Authenticated IPC confirmed ONLINE,
Rust/protocol 3, verified startup and confirmed activation. The saved instrument
remained DISCONNECTED, without responsibility, pending work or a connection ID.
Startup did not restore instrument connections. No physical zero, instrument
reset, real-device reconnect or single-pass scan qualification is claimed.

Startup inspection, process and launch evidence are retained in Result/app-freeze.
