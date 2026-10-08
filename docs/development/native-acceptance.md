# Native backend qualification

## Current status

The production chain is Rust: five instrument driver libraries, Fiber Setup,
the independent Worker, Host, archive/export, local/remote communication and
native diagnostic CLI. The Tauri GUI remains English HTML/CSS/JavaScript.
No production interpreter search, source-worker launch or Python fallback is
available. Historical Python code and experiments remain reference-only.

The candidate is **implemented and offline-tested, not deployment-qualified**.
Nothing in this migration replaces the currently installed App or authorizes
a new hardware test, vendor installation, merge or push.

| Stage | Evidence on 2026-10-07 | Status |
| --- | --- | --- |
| Native workspace | 426 Rust tests; no ignored tests | Passed |
| Frontend | 271 Node tests; no skipped tests | Passed |
| Packaging regressions | Six PowerShell tests, including Unicode paths, environment restoration and active-owner refusal | Passed |
| Native release | Three x64 MSVC executables; static CRT; no Python/VCRUNTIME DLL imports | Built |
| Portable payload | Seven-file allowlist, manifest/resource hashes, no Python or diagnostic fixtures | Passed |
| NSIS | Candidate compiled using guarded, pinned template; no installer executed | Built; installation pending |
| Formatting | Migration-owned files formatted; full `cargo fmt` still reports legacy untouched files and three pre-existing user-dirty files | Scope passed; whole-tree not passed |
| Genuine no-Python Windows | No fresh Windows computer/VM available for this run | Pending |
| Real instruments | No native enumeration, identity or action stage executed | Pending |
| Two installed Apps / actual two-PC Tailscale | Offline native flow and actual TLS finite-peer tests passed; no interactive installed-App/two-PC run | Pending |
| Installed-App migration / restart / driver cleanup | Covered by offline fixtures only | Physical/installation acceptance pending |

The old Python-backed physical OSA tests are historical, not evidence that the
rewritten Rust driver works on a real AQ6370. Removing Python from this
development shell's PATH is not a clean-Windows qualification.

The first static candidate is machine-local at
`tmp/native-candidate-static-20261007`. Its `qualification.json` records
source/package identity, protocol 3, startup revision 1 and SHA256 of the GUI,
Host, Worker and installer. It records `installed=false`,
`clean_windows_validated=false` and `physical_validated=false`. Formatting or
review fixes require a new candidate directory and new hashes. Use the exact
latest candidate and its qualification report for acceptance; do not mix files
from builds or credit an older package to a changed source tree.

Machine-local evidence belongs in a new
`Result/console/native-<run>` qualification directory, never Git. Keep separate
report fields for offline, installer, dependency, startup, identity, action,
archive/export, cleanup, restart, loopback and actual two-PC outcomes.

## Offline gate

Prepare x64 MSVC, Rust and Node as described in `../development.md`. No Python
command or package manager is used. Release any existing live instrument owner
through ordinary verified shutdown; do not kill it to make a test pass.

```powershell
./App/scripts/test-native.ps1 -Offline
pwsh -NoProfile -File App/scripts/tests/native.Tests.ps1
cargo fmt --all -- --check
```

`test-native.ps1` builds the native finite pipe fixture and diagnostics, runs
the complete workspace sequentially, then all frontend tests. It removes
Python/Anaconda/WindowsApps aliases from the child test PATH. Test adapters are
not selectable by production arguments and never packaged. Also run the
plan's `cargo test --offline --locked --workspace --all-targets --features
sil-instrument-console/host-bin -- --test-threads=1` before final qualification.

Run `check-package.ps1` on the candidate portable directory and compare all
three executable hashes to its qualification report. Inspect native PE imports
for interpreter/CRT prerequisites. The check validates the portable payload;
it does not claim that an unexecuted installer was installed or that a vendor
driver is healthy.

## Genuine clean Windows installation

Use a fresh Windows x64 VM or another real computer with **no Python, Anaconda,
Rust or Node** installed. Record Windows version, ordinary-user account, exact
installer hash and prerequisites. Install Microsoft WebView2 separately if
missing; the candidate does not silently install it, VISA or USB/serial vendor
drivers or accept their licenses.

1. Close the installed GUI, Host and Worker normally, retaining cleanup reports.
   Confirm no owner remains. The installer must refuse a live owner rather than
   kill or restart it. Do not deliberately run this check with active outputs.
2. Install the exact candidate as an ordinary user in a new empty directory.
   Repeat with spaces and Unicode in install/data paths. Never overwrite a
   historical Python installation or machine-local data as a test shortcut.
3. Start the App. Local Host startup should be automatic, empty and disarmed:
   no instrument port opened and no output change. Check Overview, Settings,
   Add New and truthful missing-VISA status. Serial and archive functionality
   must not require a VISA library just to start the App.
4. Check that the Worker is the packaged `yang-worker.exe`, with matching
   protocol/startup/package identity and actual process identity. There must
   be no Python subprocess, interpreter field, simulation fallback or source
   launch. Retain startup errors if a prerequisite is absent.
5. Exercise saved device/setup IDs, names, revisions, policies, Host data root
   and existing DPAPI-protected trust under the same Windows account. Confirm
   migrated original bytes were backed up before replacement. Future/corrupt
   records must remain intact and produce an actionable error.
6. Restart the GUI and then the Host normally. Configuration/trust must survive;
   stale identity proofs, control leases and stage baselines must not. Close
   again and retain actual Worker/Host exit and cleanup evidence separately.

Do not mark the clean-machine stage passed when only source builds or a
PATH-filtered developer computer have been tested.

## Real-device staging

Every instrument proceeds through independently approved enumeration,
read-only connection and an explicitly approved reversible action. A former
Python-driver trial or earlier output approval does not cover this native trial.
Do not touch unknown USB serials `DS7A241400216`, `MY52091763`, or the network
resource `10.229.158.143`.

Use `Code/Debugs` native diagnostics and the strict catalog DomainConfig;
experiment code never opens VISA/serial directly. CLI execution requires exact
`--execute --confirm-stage <stage>` acknowledgement. Without those flags it
prints a preview and opens nothing. Each CLI stage writes a **new absolute
`Result/<short-name>`** directory; the qualification index references those
immutable reports. No report is overwritten and no failed action is replayed.

For AQ6370, separately authorize enumeration, then read-only identity on
`GPIB0::4::INSTR`, then reading an **existing trace**. Compare the returned panel
context, sample counts, native units, raw little-endian f64 archive bytes,
SHA256 and CSV round-trip values. This stage must not issue INIT, ABORt or
front-panel setting writes. Sweep acquisition is another separately approved
action, not an automatic way to replace a failed trace read.

For other devices, disclose the following before requesting an action stage:

| Device | Required action disclosure / invariants |
| --- | --- |
| Voltage Source | Normal startup/close zeros all eight channels. Commands 0–14 V, normal steps <=0.1 V and >=50 ms; handled faults/shutdown send immediate all-zero. Read-only probe uses a distinct nonzeroing lifecycle. |
| Gain Chip Driver | Normal startup/close disables current before TEC. Current <=200 mA, target 15–40 degC; enabling requires TEC on and continuous fresh temperature within +/-0.2 degC for five seconds. Watchdog/fault cleanup remains enabled. |
| PM400 | Connect/normal close preserves panel settings. Sensor capabilities gate all dependent writes. Reset, optical zero/calibration, detector response and adapter changes need explicit action confirmation. |
| MDT693B | Read-only connect/recover/normal close holds outputs. Ceiling 75 V, steps <=0.1 V at >=50 ms. Faults stop and hold, never automatically zero. Motion needs explicit baseline authority; zero is separately confirmed. |
| Fiber Setup | Exact left/right serial binding, lab coordinates and explicit nominal MAX312D scale. Session-only estimate; toward-chip <=0.2 um, other per-axis <=1 um. Lost authority/partial failure invalidates estimate and holds both stages, never rollback/zero. |

Gain/Voltage action diagnostics additionally require
`--acknowledge-lifecycle`. If identity, output state, cleanup or release is
uncertain, record failure/unknown; do not call it a pass. An absent instrument
remains physically unvalidated. Handle release, ordered safety attempts and
host-observed zero are different claims; none is an independent physical
measurement.

## Local/remote Apps

First run two native Apps on one PC, with real Host ownership kept separate from
offline fixtures. Then repeat on **two physical computers** using the lab's
Tailscale network; label loopback and actual two-PC results separately.

Approve first-use trust once in Settings. A trusted peer reconnects after App
and Host restart without another approval. Local and connected remote devices
coexist, and the instrument's owning Host remains the only process doing I/O.
Read OSA remotely and confirm both Apps observe the same capture/operation ID,
native bytes and archive. Check navigation, drafts, previous capture, elapsed
pending feedback and export during a wait; never fabricate progress or replay
a timed-out action. Test disconnect and trust revocation, ordinary driver-led
cleanup and truthful retained-owner state before any replacement.

Only after these pending stages have actual evidence may the candidate be
called deployment-qualified. Integration/publication is a separate operator
decision; this document does not authorize main merge or GitHub upload.
