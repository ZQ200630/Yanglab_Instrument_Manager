# Development environment

Source development uses the Anaconda environment **VISA**, with Python **3.10.16**. Ordinary users will use the App's private runtime after that separate deployment work is qualified; they do not need Anaconda. Do not copy a development environment into an installer.

Run commands from the repository root in PowerShell. Anaconda/Miniconda must already be installed and `conda` available. No hardware is required for environment creation or the offline commands below.

## Create a new environment

First inspect existing environments:

```powershell
conda env list
```

If `VISA` already exists, stop and inspect it using the verification commands below. Do not remove, recreate or update it automatically: other lab programs may use it.

On a machine without an existing `VISA` environment, the preferred setup is:

```powershell
conda env create -f environment.yml
```

Alternatively, use the same Python version and the pip dependency list:

```powershell
conda create -n VISA python=3.10.16 pip
conda run -n VISA --no-capture-output python -m pip install -r requirements.txt
```

Use one route, not both. `environment.yml` declares the environment name, Python and direct dependencies; `requirements.txt` declares the same six exact pip package versions. Their semantic alignment is tested. Neither file locks the entire transitive dependency graph, conda solver, OS or vendor drivers. The installed runtime has its own separately verified complete wheel/hash lock; it is not this development list.

## Verify before development

```powershell
conda run -n VISA --no-capture-output python -B -c "import sys; from pathlib import Path; assert sys.version_info[:3] == (3,10,16); assert Path(sys.prefix).name.casefold() == 'visa'; print(sys.executable); print(sys.version)"
conda run -n VISA --no-capture-output python -B -c "from importlib.metadata import version; pins={'numpy':'2.2.4','pyserial':'3.5','PyVISA':'1.14.1','PyMeasure':'0.15.0','matplotlib':'3.10.1','Pillow':'11.1.0'}; actual={name:version(name) for name in pins}; print(actual); assert actual == pins"
conda run -n VISA --no-capture-output python -B -m pip check
conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_environment_docs -v
```

Expected: the executable belongs to `VISA`, Python3.10.16 and all six package versions match, pip reports no broken requirements, and the dependency/configuration contract tests pass. A failure needs investigation, not an automatic environment replacement. Existing-machine inspection is not proof of fresh-machine reconstruction; that acceptance remains pending until performed on an isolated target.

## Hardware-free regression and plan preview

```powershell
conda run -n VISA --no-capture-output python -B -m unittest discover -s Code/Debugs -p 'test*.py' -q
conda run -n VISA --no-capture-output python -B -m unittest discover -s App/tests -p 'test*.py' -q
conda run -n VISA --no-capture-output python -B -m Code.Experiments.sil_hysteresis.run --plan --no-display
conda run -n VISA --no-capture-output python -B -m Code.Experiments.sil_repeat.run --plan --no-display
```

Tests use explicit finite transport/storage boundaries, not a selectable instrument backend or GUI measurements. Some intentional negative tests print protocol errors; judge the final test count, failures and process exit code, not a single log line. Offline passes are not hardware acceptance.

The App suite also contains Windows native Host process tests. They require the corresponding built Host and currently have machine-specific development fixture paths, which the private-runtime launch task must make portable. A fresh Python environment alone does not guarantee those native tests can run on another computer. Do not mimic a different computer's absolute path or modify system Python to hide a failure.

Native/frontend development additionally needs Node.js, Rust 1.88 or newer (the certificate-generation dependency's minimum), Microsoft C++ Build Tools, Windows SDK and the matching Tauri prerequisites. These are not Python requirements. With the tools and caches prepared:

```powershell
node --test App/tests/*.test.mjs
cargo test --offline --locked --features host-bin --manifest-path App/src-tauri/Cargo.toml
```

Run native Host process tests and Cargo ownership tests sequentially: they intentionally share the machine-wide owner guard. Existing `App/scripts/build-host.ps1` uses this development computer's build-cache paths; it is not yet a portable installer/bootstrap script. Never install a candidate over the existing App merely to run tests.

## Build the private runtime (no installation or hardware)

`App/scripts/build-runtime.ps1` requires explicit absolute VISA Python and Git executables. Git is a build prerequisite; no Git/network access is needed by the eventual installed worker. The builder itself does not download anything, change the development environment, install an App or start a device.

Prepare a new input staging directory containing `python-3.13.16-embed-amd64.zip`, `wheels/` with the15 exact archives from `App/runtime/wheel-sources.json`, and `licenses/pyserial-3.5.txt` from `license-files.json`. Archive/package/notice hashes are pinned in interpreter.json, requirements-win-x64.lock and license-files.json; the assembler checks them and wheel RECORD/default target dependency closure. Do not silently substitute an archive, wheel, package version or backend. The runtime catalogs supplied must match the named source commit, allowing only Git LF/CRLF text checkout conversion.

For example, after assigning absolute paths appropriate to your computer:

```powershell
$taskPython = (conda run -n VISA --no-capture-output python -B -c "import sys; print(sys.executable)").Trim()
$taskGit = (Get-Command git -ErrorAction Stop).Source
$taskRoot = (Get-Location).Path
$taskCommit = (& $taskGit rev-parse HEAD).Trim()
# Assign $taskInputs to the prepared input directory, and $taskPayload to a NEW
# destination with an existing parent. Neither path is a source/installation root.
./App/scripts/build-runtime.ps1 -Python $taskPython -Git $taskGit -SourceRoot $taskRoot `
    -SourceCommit $taskCommit -InputRoot $taskInputs -Destination $taskPayload
conda run -n VISA --no-capture-output python -B -m unittest App.tests.test_runtime_build -v
```

The payload uses only selected regular blobs of that exact commit; uncommitted source, tests, experiments, data and credentials are not copied. This preserves unrelated working changes but also means new drivers must be committed before inclusion. Keep the builder's own source revision and output manifest/build hashes in qualification evidence. A failed assembly removes only its owned temporary stage and cannot replace an existing destination.

The private interpreter has four explicit contained search paths and no site/.pth execution. Trusted qualification/installed launch must use `-I -B -X utf8`: isolation alone leaves Windows pipe encoding at the system code page, which failed actual Unicode-path JSON replies. Do not rely on PYTHONIOENCODING, which isolation ignores. The package-bound native launch enforcing these flags is still the next task, not an installed feature yet. Qualifying that exact executable is the approved exception to ordinary source commands using VISA; it is not an alternate selectable backend. Isolated imports and an empty-worker lifecycle on a development computer remain distinct from native launch and a genuine Windows target without Python/Anaconda. Do not copy a sandbox-owned private build directory between Windows users or change workspace ACLs to hide an identity failure; build under the intended ordinary-user identity or use the later verified installer.

## Vendor drivers and real instruments

PyVISA is a Python package, not NI/Keysight VISA or a GPIB adapter driver. CH340/CP210x USB/serial drivers and instrument firmware are also separate. Missing vendor dependencies must be diagnosed and installed from authorized vendor sources, with applicable license/admin approval; do not install a different backend silently.

Read `AGENTS.md` before any device action. Enumeration, read-only connection and output-changing diagnostics are separately authorized stages. Removing `--plan` from an experiment selects **real hardware**, including output changes. Even connecting Voltage Source or Gain Driver can trigger safety writes. Use `Code/Utils` drivers and `Code/Setups/fiber_coupling.py`, never raw serial/VISA access from experiments or a workaround for an interlock.

## Dependency changes and machine branches

Change reviewed direct dependencies in both manifests together, run their contract and full offline regressions, and explain why the new dependency is needed. Keep machine-local addresses, configuration, credentials, build outputs and measurement data out of published source. Do not add `pip freeze` from a lab computer as an unreviewed substitute.

The operator requested source publication now so both instrument computers can develop concurrently. Use `codex/pic-desktop` for the PIC desktop or `codex/laser-1060-desktop` for the 1060 laser desktop, starting from the same `main` snapshot. Clone the repository, switch to the assigned branch, then create or inspect VISA as above. Push changes on your machine branch; the operator will request integration after both sides are ready.

This is source availability, not standalone installation or remote/hardware acceptance. The private-runtime launch is unfinished. Existing native build scripts refer to the PIC computer's build cache; on another machine configure normal Rust/MSVC/Tauri build paths rather than recreating those cache paths. Never commit machine-specific addresses, credentials, build outputs or measurement data.

## OSA and remote Apps

The OSA page uses **Connect → Read trace → Save**. Read trace retrieves an existing front-panel trace A–G; it does not start a sweep or change measurement settings. Captures retain native units, exact samples and acquisition metadata. Save exports a verified archive to a directory selected in the native App. An incomplete or stale result is not a new complete measurement.

Each ordinary App owns or attaches to its computer's local Host. Remote devices are additional devices, not a replacement local workspace. Configure networking in **Settings**, not Overview:

1. Open **Settings → Connections → Control this PC** on the instrument computer. Expand Listener settings and allow connections at its explicit Tailscale IP and an unused port, for example `100.x.y.z:9443`. The listener starts disabled; wildcard, LAN and public-address binds are rejected. Configure Tailscale and an appropriately scoped Windows firewall rule separately. Do not expose the service through Funnel or a public port forward.
2. In the other App, choose **Control other PCs → Add PC**, enter that IP (default port 9443), and **Request connection**. A nickname is optional under Details. No fingerprint or pairing-code input is needed.
3. Click **Approve** on the owning computer to trust this new request, or **Reject**. No number is displayed, entered or compared. This is the operator-selected first-use trust policy for internal lab computers, not independent identity verification: caller-supplied names/IPs and an allowed Tailscale address range are not proof of ownership. The first pairing no longer has the independent comparison check against impersonation/terminated relays.
4. Pairing persists the actual certificate and credential natively, then attempts ordinary Connect without opening any instrument or acquiring control. If Connect fails, the computer remains **Paired / Offline**; use Connect separately. Already trusted computers reconnect with **Connect**, including after App/Host restart, without another request or Approval. Future connections use the saved full certificate pin and credential; a mismatch never falls back to unverified trust or silently replaces the identity. Revoked/forgotten trust must be deliberately paired again. Disconnect/reconnect never replays a command or automatically reclaims control.
5. Computer Details contains read-only identifiers and explicit Revoke/Forget. Revoking a peer or disabling the listener closes its sessions and invokes owning-Host cleanup. Local sessions are independent. Certificates and credentials stay native and Windows-user DPAPI-protected; copying a configuration directory is not a supported identity transfer.

Requests expire after 120 seconds and remain native-owned across tab/page changes. Cancel/App close stops and awaits pending jobs. Cancellation, interrupted delivery or requester storage failure can race owner approval and leave an owner-side authorization: inspect the owner's Authorized computers → Details and explicitly revoke an orphan. The App does not claim guaranteed remote trust removal or automatically replace existing trust. The old manually pinned/code-gated wire method remains for diagnostic/older-client compatibility only; the new UI neither exposes it nor silently downgrades to an old Host.

The native v2 bootstrap and its internal channel-binding/comparison metadata remain wire-compatible with the earlier implementation. That internal number is not a current UI approval requirement and supplies no operator verification when it is not compared. Encrypted storage, bounded requests, explicit approval, saved-pin enforcement, cancellation and device-access isolation are unchanged.

After a transport failure, **Disconnect** can use a fresh authenticated observer to reconcile the original peer/Host/boot/session: it fences that old session and requests its usual driver-controlled cleanup if still needed, then checks release. It does not acquire control or replay measurement/output operations. A denied acquisition does not create ownership liability. Normal verified Host shutdown persists the latest sixteen boot-release receipts so a restarted Host can confirm an old session; an unverified crash, missing receipt, changed certificate or incomplete cleanup remains uncertain and cannot silently unlock control. After revocation the owner must explicitly reauthorize the same peer before this recovery connection is possible. An authenticated, matching Host-stop event can also establish release. None of these receipts is a physical zero measurement.

For a second native window on the **same** Windows computer, launch `sil-instrument-console.exe --network-only --profile observer`. This profile has isolated pairing settings and cannot start or attach to a local hardware Host. Pair it to the ordinary App's explicit loopback listener, such as `127.0.0.1:9443`. It contains no simulated instrument or measurement backend. This network-only profile is a client, not a second hardware owner.

The hardware-free native TLS diagnostic uses a new evidence directory and an explicitly selected freshly built Host:

```powershell
conda run -n VISA --no-capture-output python -B -m Code.Debugs.check_osa_remote --host C:/path/to/yang-lab-host.exe --out Result/console/new-link-check
conda run -n VISA --no-capture-output python -B -m unittest Code.Debugs.test_check_osa_remote -v
```

Without `--read-osa` the diagnostic starts an empty real Host and checks TLS, legacy pairing compatibility, events, restrictions, revocation and normal shutdown; it opens no instrument resource. New bootstrap behavior is covered separately by production native actual-TLS tests, including independent same-channel/terminated-relay comparisons, phase/binding mismatch, cancellation, pin/alias rejection and partial extra input. Only after the separately authorized known-OSA read stage may the operator add `--read-osa --confirm-known-osa-read`. That stage reads the existing trace on `GPIB0::4::INSTR`, compares owning-Host and remote archive bytes, and exports them; it does not start a sweep. A loopback pass is not a native two-window or two-PC/Tailscale acceptance result. Record those separately; no reachable peer means two-PC acceptance remains pending.

### PIC acceptance status (2026-10-06)

The authorized existing-trace stage passed on a real AQ6370E: 2,000 native dBm samples were read without a software-triggered sweep, queried panel context matched before/after, and local/remote TLS archive bytes and metadata matched. Sequential trace reads are still labeled **Consistency unproven**, not atomic captures. Native App display, historical reload and CSV export passed; every parsed CSV sample matched the native archive exactly. Both native App windows sharing the local Host showed the same new real trace automatically, with the non-controller remaining read-only. Normal shutdown confirmed resource release and successful worker/Host exit.

The genuine paired network-only native GUI OSA loopback workflow also passed separately: one remote existing-trace read appeared automatically in the owning local observer; both normal App exits and ordinary supervised worker/Host shutdown completed. This pre-existing hardware evidence is distinct from the new pairing UI and is not rerun by this change.

For the original comparison-based request/approve increment, the final full regression passed 239 web tests and 194 native tests. One independent whole-change review found no critical issue and one important keyboard-focus regression; its mounted countdown-refresh test failed before the fix and passed afterward, preserving the exact Approve/Reject/Cancel/Details target without invoking it. Both candidate binaries built using the cached toolchain, the hardware-free legacy compatibility diagnostic passed with verified ordinary resource/worker/Host release, and ten VISA diagnostic/dependency tests passed. Native layout inspection used two new App profiles and an empty supervised Host; both windows closed normally, resource release and successful worker/Host exit were recorded, and all three owned processes were absent. In a subsequent separate empty-Host native trial, the operator completed new GUI approval: the owner displayed the authorized viewer and the viewer displayed Paired / ONLINE. These are loopback observations, not actual two-PC Tailscale acceptance, which remains **pending**.

The approved no-number UI update passed all 241 frontend tests and a GUI-only native build; the Rust protocol and trust implementation were unchanged, and the earlier 194 native tests were not rerun for this UI-only update. Both native Apps were closed normally and reopened with their existing profiles while the same empty Host stayed running. Saved trust survived the App restart: the viewer displayed Paired / OFFLINE, then ordinary Connect reached Paired / ONLINE without any new request or Approval. This establishes native App-restart reconnection, not a newly performed Host-restart or two-PC test. Untrusted approve-only rendering and request/cancel handling have frontend regression coverage; operator-performed new approval with the no-number candidate remains pending. This latest trial is left open for operator use, so its final supervised Host shutdown is not yet claimed. Existing physical OSA evidence and immutable reports remain untouched. Machine-local reports and measurements stay under `Result/console`, outside published source. No instrument was opened by these pairing trials, and no other instrument or installed/private-runtime qualification is implied.
