# Native development

The active stack is Rust from drivers through Worker and Host. The GUI uses
Tauri plus static JavaScript/HTML/CSS. No Python interpreter or Anaconda
environment is used by the active App, diagnostics, builds or native tests.

## Windows prerequisites

Install Rust (MSVC, Rust 1.88 or newer), Microsoft C++ Build Tools with the
Windows SDK, Node.js (for frontend tests), and WebView2 (for the desktop GUI).
The build scripts require PowerShell 7 (`pwsh`).
Run Cargo from an x64 Developer PowerShell. Vendor VISA/GPIB and USB/serial
drivers are independent dependencies, not bundled Python packages.

```powershell
git clone https://github.com/ZQ200630/Yanglab_Instrument_Manager.git
cd Yanglab_Instrument_Manager
git switch codex/pic-desktop # use codex/laser-1060-desktop on the other PC
./App/scripts/test-native.ps1
```

The test script first builds the native development-only fixture and Worker,
then runs all Rust and frontend tests sequentially with Python absent from the
child process PATH. It does not enumerate or open instruments. Set
`-Offline` only when Cargo dependencies are already cached. `Cargo.toml` and
`Cargo.lock` are the active dependency manifests; keep the lockfile committed.

Source tests disable only Tauri's absent generated sidecar requirement; this
does not select another backend. Packaging must build and validate the actual
native payload. Do not install over the operator's App to run tests.

## Candidate packaging (no installation)

Install the reviewed Tauri CLI 2 as a development tool. The scripts never install
or download it automatically. In an x64 Developer PowerShell 7 session:

```powershell
$taskSource = (Get-Location).Path
$taskCargo = (Get-Command cargo -ErrorAction Stop).Source
$taskTarget = Join-Path $taskSource 'target'
$taskCandidate = Join-Path $taskSource 'tmp/native-candidate'
# tmp must already exist; native-candidate must not exist.
./App/scripts/build-package.ps1 -SourceRoot $taskSource -Cargo $taskCargo `
    -TargetDir $taskTarget -Output $taskCandidate
./App/scripts/check-package.ps1 -Root (Join-Path $taskCandidate 'portable') -SourceRoot $taskSource
pwsh -NoProfile -File App/scripts/tests/native.Tests.ps1
```

Add `-Offline` with prepared Cargo/NSIS caches. Output contains a portable native
folder, candidate NSIS installer and separate `qualification.json`. No candidate
is installed or launched by these commands. Build environment variables are
restored on success and failure. An existing candidate output is refused.
Native package builds statically link the Microsoft C runtime; verify PE imports
to ensure no private VC/Python runtime environment is needed on the target.
The source identity is a content hash of the actual compiled input tree, not
a claim that an uncommitted working tree equals a Git commit. Native sidecars
and generated payload/identity/notices remain ignored build outputs.

Startup pins `native-package.json` and the fixed `yang-worker.exe` hash; it never
searches PATH or interpreters. qualification.json is post-build evidence for
all three executables and installer, not an embedded whole-App security promise.
Package allowlists reject Python source/interpreters/wheels/manifests, diagnostic
fixtures, credentials, recordings, source launchers and caches.

The installer refuses a live App/Host/Worker and holds the machine owner during
replacement. It never force-closes them or automatically runs an old uninstaller.
An old Python install requires a new empty install directory after normal
shutdown; historical files/data are not deleted or migrated automatically.
WebView2 must already be present. Vendor VISA/GPIB/serial dependencies are checked,
not installed and their licenses are not accepted. Serial devices and archived
data remain usable when the VISA dependency is unavailable.
Actual clean-Windows installation and upgrade behavior still need qualification
on the exact candidate. Do not equate script/content tests with an installation.

## Structure and safety

- `Code/Utils/src`: typed native instrument drivers.
- `Code/Setups/src`: logical fiber coordinates, serial binding and session estimates.
- `Code/Debugs/src`: explicitly staged native diagnostics.
- `App/worker-rs`: isolated native Worker; `App/src-tauri`: Host and GUI.
- `Code/Experiments/<name>`, `Result/<name>`: experiments and machine-local outputs.

Read AGENTS.md. A language migration does not authorize hardware actions.
Enumeration, read-only identity checks and reversible actions require separate
operator approval. Voltage/Gain normal startup and close change output state;
a diagnostic read-only probe does not run that lifecycle. MDT/Fiber connect
and normal close hold all outputs. All device limits remain inside the drivers.

## Native diagnostics

`yang-debug.exe` with no arguments prints help and opens nothing.
Every command defaults to preview. Execution needs both `--execute` and
`--confirm-stage` matching the explicitly selected stage.

```powershell
$taskOut = Join-Path (Get-Location) 'Result/enum-check'
# Preview only:
cargo run --locked -p yang-debug --bin yang-debug -- enumerate --stage enumerate --out $taskOut
# After separate enumeration approval, add --execute --confirm-stage enumerate.
```

Device commands are `osa`, `voltage`, `gain`, `pm400`, `mdt`, `fiber`.
Supply `--binding` as a strict DomainConfig JSON object (domain, config_rev,
driver_kind, model_id, profile_id, params, expected_identity, members), using
the trusted catalog's connection profile. A new absolute `Result/<short-name>`
directory is mandatory; existing evidence is never overwritten.
`--stage readonly` performs identity/state probing and confirmed cleanup only.
`--stage action --actions '[{"name":"read_trace","args":{"trace":"A"}}]'`
runs 1–16 typed catalog actions with fresh identity checks, no raw commands or
automatic retries. Gain/Voltage additionally require
`--acknowledge-lifecycle` after the operator has approved startup/shutdown
effects. Fiber movement needs baseline adoption and explicit nominal-scale
authorization in the same session; saved estimates never establish authority.

Reports include actual per-attempt cleanup evidence, including partial failure
and uncertainty. An unreleased local owner stays alive and retries cleanup,
not commands. Never force-kill an active diagnostic to hide a stalled release.
A release receipt is not a physical zero measurement.

`remote --stage readonly --peers <absolute peers.dpapi> --host <Host ID>`
uses existing user-protected App trust; optionally `--archive <reference.json>`
downloads an already archived capture as exact native bytes and manifest.
It never pairs, creates a Host, acquires control or opens an instrument.
First-use pairing remains in Settings, with explicit owner approval.
Local and remote devices coexist; connection failures never replay commands.

## Responsive interaction and deployment status

Keep I/O, storage and decoding off the GUI interaction path. Acknowledge
actions immediately; show pending/elapsed time, explicit errors and uncertain
outcomes. Only show percentages from actual byte/work progress. Preserve
drafts/focus/navigation and label previous/historical data accurately.

See [native parity](development/rust-driver-parity.md) for implemented,
offline-tested and physically validated status. Earlier Python-backed OSA
acceptance is historical evidence, not validation of the rewritten Rust driver.
Clean Windows installation and physical/two-PC acceptance must be recorded
separately; a PATH-filtered development test is not a clean VM acceptance.

## Preserved legacy Python

Old `.py` drivers, workers, tests, assembly scripts and experiment entry points
are reference-only, not launched or migrated automatically. Their reviewed
environment pins remain in requirements.txt/environment.yml.
[Legacy instructions](development/legacy-python.md) explain how to inspect
those references using VISA without replacing an existing environment.
Do not package them, publish machine-local data/credentials, or silently
resume historical experiments. Publish/merge only when the operator requests.
