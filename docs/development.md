# Native development

The active driver → Worker → Host path is Rust. Tauri uses static JavaScript,
HTML and CSS views. Python/Anaconda is not an App, runtime, build or native-test
prerequisite. Preserved Python source/tests and their dependency pins are
historical references, never a selectable backend or automatically invoked migration.

## Prerequisites

Use installed Rust MSVC 1.88+, Microsoft C++ Build Tools with Windows SDK,
Node.js, PowerShell 7 and WebView2. Prepare an x64 Developer PowerShell before
Cargo commands. Cargo.toml/Cargo.lock are authoritative; use --locked and an
already populated cache for --offline. Scripts never install tools or replace
an environment. Vendor VISA/GPIB and Newport/USB/serial dependencies are separate.

~~~powershell
./App/scripts/test-native.ps1 -Offline
~~~

This builds yang-worker and yang-debug finite fixture binaries, runs the whole
Rust workspace sequentially with host-bin, all Node tests and native package
tests. Python/Conda is removed from child PATH. Explicit transport injection is
test-only; no fixture enters the catalog or production backend. No instrument
enumeration/open is part of this suite. Tests use random pipe/TLS/mutex namespaces,
not the operator's running Host. Do not run historical Python process tests as
part of native qualification.

## Native package candidate

Both package routes build the root locked workspace with static MSVC CRT.
The native source fingerprint includes TLB source/manifest, model table, all
driver pins/resources/provenance and active build scripts. It recomputes its
file set and refuses source additions or modifications during Worker/Host/GUI
build and package staging. It represents the actual input tree; it does not
pretend an uncommitted tree is a published Git commit.

From a prepared x64 Developer PowerShell 7:

~~~powershell
$taskSource = (Get-Location).Path
$taskCargo = (Get-Command cargo -ErrorAction Stop).Source
$taskTarget = Join-Path $taskSource 'target'
$taskParent = Join-Path $taskSource 'Result/native-package'
New-Item -ItemType Directory -Path $taskParent -Force | Out-Null
$taskCandidate = Join-Path $taskParent ('candidate-' + [guid]::NewGuid().ToString('N'))
./App/scripts/build-package.ps1 -SourceRoot $taskSource -Cargo $taskCargo -TargetDir $taskTarget -Output $taskCandidate -Offline -PortableOnly
./App/scripts/check-package.ps1 -Root (Join-Path $taskCandidate 'portable') -SourceRoot $taskSource
~~~

PortableOnly uses installed Cargo directly and reports no installer/hash.
It is a packaging option, not an instrument backend choice. Omit PortableOnly
for the existing NSIS route only with Tauri CLI 2 and its NSIS tool cache already
prepared. Missing tooling fails before build rather than downloading/installing.
The default Tauri route retains reviewed ownership hooks, WebView2 prerequisite,
current-user install mode and refusal to replace live App/Host/Worker processes.
It never force-closes an owner, accepts vendor licenses or automatically runs
an old uninstaller. A legacy Python install requires a new empty folder after
ordinary shutdown; old files/data are not deleted or auto-migrated.

The common portable/NSIS contract has 29 exact files: three native executables,
manifest/notices/catalog/Fiber configuration, fixed driver manifest, 18 pinned
vendor files and three provenance READMEs. Package checks reject extra packages,
files/directories, reparse paths, changed approved bytes and duplicate manifest
keys. TLB is linked into Worker; its CLI/Python bridge/model source are not payloads.
The CP original license/release notes and manufacturer redistribution attestation
remain preserved. No driver is installed at App startup or App installation.

Native launch pins the fixed Worker image and manifest; strict startup checks
the image/package identity before durable ownership recording and one-use activation.
Local and TLS clients require compiled native Host implementation capabilities
(worker_kind=rust, worker_startup_revision=1). These markers are independent
of worker_startup_verified and worker_activation_confirmed evidence. A failed
native Host stays reachable for disarmed management while action fences remain.
The old real/protocol-3 Python Host is rejected and never stopped/replaced automatically.

Generated native sidecars, notices/identity, target and candidate folders are
ignored outputs. An existing candidate is refused and caller build environment
is restored on success/failure. qualification.json records actual file hashes
and whether an installer was built; physical, clean-Windows and installed flags
remain false until separately verified. Structural finite MZ fixtures are
only checker/build-script tests and cannot qualify runtime execution.

Inspect actual candidate PE architecture/imports/static runtime and the exact
packaged disarmed Worker startup/EOF with bounded timeouts, no activation,
configuration, inventory, SDK scan, connection or command. Do not start production
Host/GUI against user records to qualify a candidate. SDK loading is lazy and
is not required for this hardware-free startup proof. Clean-Windows installation,
UAC/vendor installation and physical acceptance remain separate work.

## Instrument and data rules

Read AGENTS.md. Enumeration, read-only connection and a reversible output action
are separately authorized. Source/Gain connection may perform safety writes.
Use Code/Utils drivers and the native Code/Setups Fiber interface, with exact
serial-side binding, lab coordinates and unchanged limits. Do not use raw SCPI,
serial/VISA bypasses, force-kill retained responsibility or replay uncertain calls.

Source: 0–14 V; normal steps ≤0.1 V/50 ms; failures/shutdown immediate zero.
Gain: 0–200 mA, 15–40 °C; TEC/temperature five-second stability before current;
current off before TEC at shutdown. MDT: ≤75 V, 0.1 V/50 ms; fault stop-and-hold,
zero only explicitly authorized. Fiber: toward-chip ≤0.2 um and other per-axis
≤1.0 um; estimate invalidation on authority loss/partial failure, no rollback.
OSA/PM400 preserve front-panel settings and sensor/action capability gates.

Resource release, lifecycle success and host-observed zero are distinct software
claims, none an independent physical measurement. Keep credentials, bindings and
measurements local. Capture/archive/recovery preserves actual samples, hashes,
origins and immutable cleanup evidence. Remote instrument ownership stays on its
authenticated owning Host; client loss never authorizes another controller.

[TLB integration](tlb6700.md) describes the linked Bus, limits and prerequisite
metadata. [Rust parity](development/rust-driver-parity.md) records other drivers.
[Legacy Python guide](development/legacy-python.md) is historical only. If the
operator specifically requests a legacy Python check on this machine, use the
existing D:/Program/Anaconda3/envs/VISA/python.exe; never auto-create or replace it.
