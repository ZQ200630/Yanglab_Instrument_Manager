> Operator priority update (2026-10-06): publish the current development source first for parallel work on the two lab computers. Use proportionate targeted tests; defer exhaustive package hardening. Earlier publication gates below no longer block source synchronization, but standalone-install, hardware and remote acceptance are still unproven. Do not restart completed tasks or imply these gates have passed.

# Private Windows Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. The operator selected inline execution. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a self-contained Windows x64 console, document its separate VISA development environment, and publish the audited common framework only after clean-machine and remaining framework acceptance.

**Architecture:** Preserve Rust GUI/Host ownership and the real-only supervised Python worker. Assemble a private, versioned CPython payload from verified archives/wheels; native launch validates package identity instead of an external Anaconda path. Development, deployed-runtime, hardware and remote-network acceptance remain separate gates.

**Tech Stack:** Tauri2/NSIS, Rust, PowerShell, CPython3.10.16 development VISA; candidate CPython3.13.16 standard-GIL Windows x64 embeddable runtime.

**Spec:** docs/superpowers/specs/2026-10-06-private-runtime-design.md (approved 2026-10-06); retain docs/superpowers/specs/2026-10-06-real-console-design.md safety/lifecycle requirements.

## Global Constraints

- Ordinary source Python commands and offline regressions run in Anaconda VISA. Explicit qualification runs the shipped private executable, never external Python as its substitute.
- No hardware opening/output changes during packaging or clean launch. Existing separately authorized diagnostic stages and every driver/setup limit remain enforced.
- Installed launch ignores external Python discovery/environment/user packages; no bare python/py/pip, downloads, source fallback or arbitrary frontend executable inputs.
- Runtime manifest version1; Windows x64 only; exact interpreter/package/file versions and SHA256; native build binds to expected manifest digest. No claim that unsigned hashes authenticate an entire replaced application.
- Physical owner guard, IPC authentication, nonce/child identity, bounded queues, safety lanes and retained-resource handling remain intact; no unknown-owner kill, replay or automatic output restoration.
- Original settings/results/credentials/reference_code/history remain recoverable; upgrades/uninstall preserve mutable data by default. No vendor-license acceptance, system VISA/PATH change or Tailscale change.
- Clean Windows acceptance requires a fresh VM/actual computer without Python/Anaconda/development tools; hiding PATH on this computer is auxiliary evidence only.
- Publish audited source to https://github.com/ZQ200630/Yanglab_Instrument_Manager.git only after all common gates. Create codex/pic-desktop and codex/laser-1060-desktop from the SAME verified remote main commit. Never force-push or blindly publish laboratory history.

## Review Focus

1. Installation under Unicode/spaces or a replaced link must resolve contained immutable files or reject launch, never another interpreter (Tasks2/3).
2. Corrupt/future/partially migrated preferences must preserve original bytes and show a repair state, never silently discard devices or use old Python paths (Task4).
3. Injected PYTHONPATH/sitecustomize or a native dependency absent on a remote-viewing GUI must not run external code or disable unrelated capabilities (Tasks3/4).
4. Upgrade while an owner is retained must leave the old payload/data/process untouched; concurrent installers must not replace each other's package (Task5).
5. A dirty/public export with excluded data anywhere in history must not publish it; branch creation must not race a changed main baseline (Task7).

## Files and verification conventions

- Root requirements.txt/environment.yml/docs/development.md own developer setup; runtime dependencies live under App/runtime, not Code/Utils.
- App/scripts/runtime_lock.py and runtime_assemble.py own bounded, build-only package validation/assembly; App/scripts/build-runtime.ps1 orchestrates explicitly supplied paths. No execution on ordinary App startup.
- App/src-tauri/src/package_runtime.rs owns native immutable-package admission; App/worker/runtime_identity.py owns worker identity verification.
- App/src-tauri/src/runtime.rs, host/service.rs, bin/yang-lab-host.rs and gui.rs adapt existing launch without restructuring driver code.
- App/src-tauri/src/preferences.rs owns schema migrations; existing Host registry and Settings consume it.
- App/scripts/installer-hooks.nsh owns upgrade/uninstall guards; check-package.ps1 verifies exact payload.
- App/scripts/release_audit.py and docs/release.md own non-mutating publication audit and operator-facing conditional release procedure.
- Development commands below use `VISA -B -m ...`, meaning the verified Anaconda VISA python.exe, not a new executable on PATH. Rust command is `cargo test --offline --locked --features host-bin --manifest-path App/src-tauri/Cargo.toml`; retain the known VS/cache environment. Node command is `node --test App/tests/*.test.mjs`.
- Every task keeps RED/GREEN output, exact source range and rulings in its plan-scoped ledger; scoped commits exclude unrelated .gitignore. Full suite counts must be discovered from terminal output, never assumed.

### Task 1: Publishable developer environment contract

**Files:** Create requirements.txt, docs/development.md; modify README.md, AGENTS.md, Code/Debugs/test_environment_docs.py; preserve existing environment.yml entries.

**Interfaces:** requirements.txt contains exactly numpy==2.2.4, pyserial==3.5, PyVISA==1.14.1, PyMeasure==0.15.0, matplotlib==3.10.1, Pillow==11.1.0. environment.yml retains name VISA, Python3.10.16, pip and the same six pins. Documentation distinguishes direct pins from a complete transitive lock and vendor drivers.

- [x] Write `test_requirements_match_development_manifest`: parse actual requirement entries, assert exact reviewed name/version pairs, no duplicate/unpinned/include/URL/path entries and semantic equality with environment.yml; add parser negative cases for conflicting pins and case-normalized duplicates. Preserve AST import-coverage tests.
- [x] Run `VISA -B -m unittest Code.Debugs.test_environment_docs -v`; expect RED for missing requirements.txt, with valid test collection.
- [x] Add the files and commands: `conda env create -f environment.yml`; alternative `conda create -n VISA python=3.10.16 pip`, `conda run -n VISA python -m pip install -r requirements.txt`; verify interpreter identity/package versions and `conda run -n VISA python -m pip check`. Stop if VISA already exists; no automatic remove/update. Document real-only/hardware-free regression boundary and Windows vendor dependencies. Amend AGENTS only to clarify explicitly qualified private-runtime commands, not weaken ordinary VISA/safety rules.
- [x] Run focused tests and full Code/App suites. Inspect existing VISA with metadata/pip check without installing anything; record any existing mismatch separately. Document fresh developer-environment creation as pending until actually tested in an isolated target.
- [x] Commit only these files: `docs: specify reproducible VISA development setup` (actual scoped commit dab5705).

### Task 2: Verified runtime inputs and deterministic payload

**Files:** Create App/runtime/interpreter.json, requirements.in, requirements-win-x64.lock, python313._pth, THIRD_PARTY.md; App/scripts/runtime_lock.py, runtime_assemble.py, build-runtime.ps1; App/tests/test_runtime_build.py. Generated runtime/staging stays ignored outside source drivers.

**Interfaces:** `validate_lock(lock_bytes: bytes, wheel_metadata: list[dict]) -> dict` returns exact name/version/hash/tag closure; `assemble_runtime(archive: Path, wheels: Path, lock: Path, destination: Path, source_commit: str) -> dict` creates a NEW destination and version1 manifest only after validation. Limits: manifest <=4 MiB/20000 files, <=2 GiB expanded payload; normalized relative paths <=512 characters, no duplicate case-folded destinations, links, traversal or executable .pth code.

- [x] Write RED tests for hash mismatch, absent transitive wheel, wrong cp313 ABI/architecture, sdist, duplicate names, UTF8/spaces paths, traversal/case collision, extraction limits, existing destination and partial extraction failure; original archive/destination must remain untouched on rejection.
- [x] Run `VISA -B -m unittest App.tests.test_runtime_build -v`; expect RED at missing assembly/lock API.
- [x] Pin interpreter CPython3.13.16 archive `https://www.python.org/ftp/python/3.13.16/python-3.13.16-embed-amd64.zip`, official SHA256 `97dae5274cc54867065e8d5a3226e48c35017ed332a0fdb0e27d5b5821961297`. Initial direct runtime pins are numpy2.2.4/PyVISA1.14.1/pyserial3.5/PyMeasure0.15.0; resolve their complete DEFAULT dependency closure with VISA pip's explicit cp313/cp313/win_amd64/only-binary target, record exact versions, official PyPI wheel hashes, METADATA edges/tags and licenses into the lock BEFORE assembly. No extras, pyvisa-sim, dev tests, Qt application or silently substituted package versions. Missing compatible closure blocks this candidate; inspect the failure before a separately recorded design ruling.
- [x] Implement the named APIs using bounded ZIP/RECORD inspection and explicit wheel-layout mapping, native DLL/license retention, deterministic manifest relative-path inventory and atomic final destination admission. `_pth` is python313.zip, runtime directory, Lib/site-packages and the verified package root; do not enable user-site or arbitrary .pth execution. Actual headless imports must prove this layout works; don't remove required default PyMeasure dependencies because they seem unused.
- [x] Run focused tests; download into explicit build staging only, verify official archive/hash + locked wheel closure, assemble and run the private executable for headless worker/driver imports and no-hardware ping/shutdown. Record versions, search paths and DLL failures. Candidate failure is not external-Python fallback; qualify before replacing launch.
- [x] Commit reproducible scripts/lock/licenses, not generated payload; actual assembler implementation commit284d95c (inputs6a52b71). Its exact source snapshot passed ordinary-user private imports and empty-worker lifecycle under a Unicode/spaces destination with trusted `-I -B -X utf8` flags; generated bytes/evidence remain local.

### Task 3: Package-bound native launch and worker identity

**Files:** Create package_runtime.rs/runtime_identity.py and App/tests/test_runtime_identity.py; modify lib.rs, build.rs, Cargo.toml, runtime.rs, worker.rs, host/service.rs, bin/yang-lab-host.rs, gui.rs, App/worker/main.py and native launch fixture tests.

**Interfaces:** `RuntimeDescriptor::{Installed(VerifiedRuntime), DevelopmentVisa{python,root}}`; `VerifiedRuntime::resolve(executable: &Path, expected_manifest_sha256: &str) -> Result<Self,HostError>` returns contained package/interpreter/worker paths + build identity. Development variant requires explicit development-runtime feature; official installer builds without it. `runtime_identity.verify(executable, package_root, build_id, manifest_sha256) -> dict` reports exact installed identity or rejects; no physical actions.

- [ ] Add Rust/Python RED tests for absent/mixed/tampered payload, path-link escape, wrong bitness/version/handshake, injected PYTHONHOME/PYTHONPATH/sitecustomize, concurrent owner and retained shutdown. Assert no child spawn on failed admission and no dependency on a directory named VISA. Keep exact old development identity checks in development mode.
- [ ] Run focused native/Python tests; expect RED before new descriptor exists, not from missing vendor hardware.
- [ ] Bind manifest digest via build.rs build input and validate bounded manifest/files/containment through native pinned handles before launch. Construct installed descriptor from executable location, never frontend paths/CLI root. Host validates independently. Explicit developer CLI uses --dev-root/--dev-python only in feature-enabled builds; shipped --python/--root rejects before ownership activation. Scrub Python-influencing environment, launch explicit private executable with trusted `-I -B -X utf8` flags and hidden console/stdin/stdout framing. Mandatory UTF8 is a Task2 empirical ruling: isolated Windows defaults used cp1252 and lost Unicode-path JSON replies; no reliance on PYTHONIOENCODING.
- [ ] Verify private identity in worker handshake before activation; preserve protocol/context/nonce, one machine owner, durable child record, shutdown deadlines and honest retained evidence. Worker startup is disarmed/empty and never enumerates/opens hardware. Broken runtime fails with a repair error rather than restarting unknown owners or trying source/PATH.
- [ ] Run Code/App/Node/Rust full suites and actual assembled-private ping/configure-empty/stop lifecycle. Host process tests run separately from Cargo ownership tests; confirm all owned children exit, no hardware and no developer sources imported.
- [ ] Commit `feat: launch only the verified application runtime`.

### Task 4: Safe migration, Settings and dependency readiness

**Files:** Create preferences.rs/App/tests/test_runtime_health.py; modify gui.rs, host/registry.rs, host/contracts.rs, host/service.rs, App/worker/inventory.py, App/web/setup.js, console-ui.js, host-client.js and corresponding App/tests/*test.mjs/native tests.

**Interfaces:** `migrate_preferences(original: &[u8]) -> Result<PreferenceMigration,HostError>` produces versioned validated preferences and original-byte backup intent; Host registry migration likewise preserves its existing device/revision fields. Read-only `runtime_diagnostics` returns runtime kind/build/version and capability statuses available/unavailable/unknown with concise reasons; never performs VISA/serial enumeration or open as health testing.

- [ ] Write RED tests for valid legacy migration preserving exact settings/devices/data-root/revisions, future/corrupt schemas, atomic backup failure, concurrent saves and old Python-path injection. Add native/Node tests proving installed Host starts automatically with no Python input; error state retains configuration and offers repair, not a blank reset.
- [ ] Run focused tests, expect RED at missing migration/readiness API. Include no-VISA/read-only capability tests where serial/remote viewing remain available and no raw transport construction occurs.
- [ ] Implement atomic backed-up version migration; keep obsolete interpreter field only as inert legacy metadata. Remove Python path from Settings/start/save contracts, show immutable runtime identity in Diagnostics. Only explicitly feature-enabled developer launch can use VISA paths; never a shipped GUI selector.
- [ ] Implement dependency metadata inspection without loading/opening hardware libraries to test a device. Missing vendor backend is actionable unavailable VISA capability, not global App failure or Anaconda demand. Official vendor installation links and native license/admin requirements are documented, not auto-installed.
- [ ] Run full four suites/private-runtime tests; commit `feat: migrate settings and report runtime readiness`.

### Task 5: Offline installer and guarded upgrade/uninstall

**Files:** Modify tauri.conf.json, check-package.ps1; create installer-hooks.nsh, App/tests/test_installer_policy.py and docs/install.md; extend native package/owner tests.

**Interfaces:** Tauri resources include exact private payload/manifest/licenses; Windows WebView2 install mode is offlineInstaller. Native `--prepare-update` returns a bounded machine-readable permit only after verified ordinary Host shutdown and released ownership, otherwise a refusal; NSIS hook has no force/kill branch. Native `--verify-package` is non-activating. Candidate installation uses an explicit separately verified destination/registration.

- [ ] Add RED tests proving active/unknown/retained owner rejects replacement without file/process changes, stale permit cannot be reused, concurrent installer guard excludes second mutation, wrong candidate destination refuses and uninstall preserves configuration/results/credentials by default. Exercise actual bounded guard state with finite owned processes, not only source-string assertions.
- [ ] Run focused tests; expect RED on missing update admission, not an actual installation on this lab computer.
- [ ] Implement guarded verified shutdown/update, exact versioned payload/resource mapping, offline official WebView2 prerequisite and necessary Microsoft redistributable inventory. Hash/verify installer inputs and payload; no pip/network resolution at install/start. Preserve vendor prerequisites licensing and user data; explicitly disclosed credential-removal choice is separate from default uninstall.
- [ ] Build GUI/Host/NSIS candidate, verify exact compile-input/resource/runtime manifest/hash/license inventory and no retired backend/test/source escape. Record measured package size and prerequisite versions. Do not install over the original App or count build as GUI acceptance.
- [ ] Run full four suites, package checks and source-independent private lifecycle; commit `build: package offline runtime and guard upgrades`.

### Task 6: Actual clean Windows qualification

**Files:** Create docs/clean-windows-checklist.md; update docs/acceptance.md; evidence Result/console/<qualification-name> stays local and excluded from public source.

**Interfaces:** Checklist records candidate commit/build/hashes, Windows OS/build/x64, installed prerequisite inventory, exact destination, native GUI interaction/screenshots, process identities and each passed/failed/pending gate. No clean target available means pending, not a manufactured test result.

- [ ] Record preflight proof on fresh Windows VM/actual PC: no Python/Anaconda/devtools/native VISA, ordinary-user session, sufficient storage, explicit candidate path. Creating/enabling VM/Windows features or obtaining a second computer requires that actual facility, not an assumption of availability.
- [ ] With network unavailable install candidate and bundled official prerequisites; open native GUI, confirm one local Host/private worker, persist benign local configuration/data-root, close/reopen and safe stop without opening instruments. No simulation mode/data.
- [ ] Repeat with injected Python-like environment/user-site paths; verify no external Python/pip/development path/runtime download. Missing native VISA displays precise guidance; GUI and unrelated capabilities remain functional.
- [ ] Upgrade/reinstall/uninstall isolated candidate with test data/settings; prove preservation and active/retained-owner refusal. Record failures, exact bytes and truthful cleanup evidence without physical-output claims.
- [ ] Re-run final regressions, request one whole-plan review per executing-plans, fix Critical/Important issues through TDD, commit evidence/docs. Clean runtime, hardware and two-computer remote acceptance remain distinct.

### Task 7: Conditional audited publication and two-machine branch baseline

**Files:** Create release_audit.py, App/tests/test_release_audit.py, docs/release.md; publication repository is a NEW isolated explicit directory, not rewritten original worktree/history.

**Interfaces:** `audit_export(source: Path, destination: Path, gate_report: Path) -> dict` is read-only until a fully checked new-destination export is admitted. Exact source allowlist: Code Python sources (exclude generated files), selected machine-independent Config templates, App source/catalog/scripts/locks/licenses (exclude generated binaries/payload), tests, docs, README, requirements.txt, environment.yml and reviewed AGENTS/gitignore. No reference_code, Result, local config/credentials, .aws/.codex/.superpowers, build/cache/binaries; uncertain license/secret input is a blocker, not an automatic cleanup.

- [ ] Write RED tests for excluded files in history, secrets without printing their contents, include-root traversal/links, generated executable false inclusion, dirty unrelated user files, missing gate and changed main. Expect no push/ref mutation on failed audit; verify an allowed requirements/environment/development guide survives export unchanged.
- [ ] Run `VISA -B -m unittest App.tests.test_release_audit -v`; expect RED at missing audit API. Implement exact allowlist/hash/provenance report and traced clean export retaining original history untouched. Source audit can run early, publication cannot.
- [ ] Require all runtime tasks and separate telemetry/TLS/multi-Host/available-real-driver/data-page gates genuinely passed; unresolved hardware remains disclosed and cannot be called accepted. Resolve native HTTPS Git prerequisite/authentication without silently changing credentials/global config; no secret logging. Inspect actual destination repository/refs again.
- [ ] Create a clean publication commit only after privacy/licensing review. Push normally to main, verify remote commit/tree against audited export; if nonempty divergent remote appears stop for integration direction, never overwrite it.
- [ ] Create codex/pic-desktop and codex/laser-1060-desktop explicitly from that SAME verified main SHA; check both remote refs. If main moved, do not reinterpret baseline or merge branches automatically. Provide clone/checkout/environment steps and machine-specific local-config rules; later integration awaits the user's branch-ready message.

## Self-review and execution status

All specification sections have a task: development contract1; payload/provenance2; package/owner identity3; migration/readiness4; upgrade/offline prerequisites/data5; genuine clean target6; conditional publication/branches7. Review Focus inputs are covered in their named task tests. The manifest/descriptor/migration interfaces have one owner; installed/runtime qualification never becomes a user-selectable backend. Exact interpreter archive is pinned now; the full wheel closure must be pinned and independently verified in Task2 before assembly, not solved at user startup. Official input sources: https://www.python.org/downloads/release/python-31316/ and https://pypi.org/project/numpy/2.2.4/; current PyMeasure default dependency metadata also requires pandas/pint/pyqtgraph, so four direct imports are not a complete runtime.

Plan status: approved inline execution. Tasks1/2 are complete (dab5705; inputs6a52b71/assembler284d95c). Task2's exact committed resource snapshot passed ordinary-user imports/empty-worker lifecycle under a Unicode/spaces path with mandatory UTF8;31 focused tests and full Code/App/Node/Rust gates passed. Tasks3..7 remain pending, beginning with package-bound native admission/launch. No installation/push/branches or clean-machine success is claimed. The independent telemetry increment's uncommitted primitives are preserved and do not count as completed recording integration.
