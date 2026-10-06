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

Native/frontend development additionally needs Node.js, Rust, Microsoft C++ Build Tools, Windows SDK and the matching Tauri prerequisites. These are not Python requirements. With the tools and caches prepared:

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
