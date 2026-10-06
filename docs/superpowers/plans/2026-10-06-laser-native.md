# Laser Native Qualification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and qualify the existing TLB-6700 desktop integration on the laser computer.

**Architecture:** Preserve GUI → native Host → real v3 worker → reusable driver. Replace only machine-fixed build/test paths, then exercise finite offline inputs before authorized real read-only acquisition.

**Tech Stack:** PowerShell, Rust/MSVC, Tauri 2, Node.js, Python3.10.16 in VISA.

**Spec:** `docs/tlb6700.md`, `docs/development.md`, and operator authorization on 2026-10-06 to install the missing toolchain and continue convergence.

## Global Constraints

- Keep the assigned `codex/laser-1060-desktop` branch and existing dependency pins.
- Ordinary Python commands use VISA. Never recreate an existing environment.
- Enumeration and read-only connection are authorized; output-changing diagnostics are not.
- Do not install a candidate over an existing console, infer control limits for `6722-P`, or claim standalone/private-runtime acceptance from a development build.

## Review Focus

- Missing toolchain must fail with an actionable error, not select a different backend.
- Build directories may contain spaces; pass paths as arguments, not composed commands.
- Offline mode must forbid dependency downloads and preserve the caller's environment.
- Native fixtures must resolve a repository-local target or an explicit artifact path without recreating another machine's paths.
- Real native-host tests must retain staged authorization and release all resources without changing output.

## Task 1: Portable native build and process fixtures

**Files:** `App/scripts/build-host.ps1`, `App/tests/host_build_fixture.py`, `App/tests/test_native_build.py`, `App/tests/test_local_host_process.py`, `docs/development.md`.

**Interfaces:** `native_host_binary(environ=None, root=ROOT)` returns an absolute development Host path. Explicit `YANG_LAB_TEST_HOST` takes precedence; otherwise use `CARGO_TARGET_DIR` or `App/src-tauri/target`, debug profile.

- [x] Write tests for default, relative target, explicit absolute path, and rejected relative explicit artifact; run them to demonstrate the missing resolver.
- [x] Implement the resolver and portable build script using installed Cargo and vswhere; preserve `--locked` and optional `--offline`.
- [x] Verify resolver tests, build the Host, run Cargo tests and the complete App suite sequentially because they share the owner guard.

## Task 2: Desktop and read-only native qualification

**Files:** `Code/Debugs/check_tlb6700.py` or a focused companion diagnostic, native/JS laser contract tests, `docs/tlb6700.md`.

- [x] Build the Tauri executable and verify sidecar resources originate from this source revision.
- [x] Verify the laser model, identity proof, driver metadata and read-only sample through the real native Host, using an owned temporary registry and the VISA worker.
- [x] Preserve the key-off state and report resource release separately from controller telemetry.
- [x] Update qualification evidence, review the final change, and prepare only source and documentation for the assigned branch's publication.
- [ ] Complete final reviewed-artifact hardware acceptance: the latest read-only
  attempt failed at connection readback with `COMMAND NOT VALID`; preserving
  release/worker exit/Host exit were confirmed. The operator subsequently
  reconnected USB, but the native path and installed official managed SDK still
  reproduced the error. Three unaccepted framing experiments were reverted.
  Further controller-state diagnostics need their own authorization; do not
  expand retries or change output to mask the error.

## Execution and final review record

The native toolchain was installed with the operator's explicit approval. Debug
Host and desktop builds succeeded; Host/sidecar hashes match and selected copied
driver/worker/catalog resources match current source. The owned native Host
completed identity-bound connection and three distinct real read-only samples,
then confirmed resource release, worker exit and Host exit. Ignored Result logs
retain earlier protocol failures and partial-attempt evidence.

The fresh whole-branch review reported two Important findings and no Critical or
Minor findings. One fix pass reproduced and corrected frozen cached laser ages
and build-script environment leakage. Cache age advances during a blocked next
observation without issuing hardware getters. Finite successful/failed builds
preserve changed, added, removed and empty process variables in Windows
PowerShell5 and the current explicitly selected Core runtime. An actual offline
Host build separately confirmed complete caller environment preservation.

Rulings, retained from this plan's executor ledger:

- Use native PowerShell/VISA bookkeeping because bundled Bash helpers are
  unavailable, preserving task/review boundaries without another installation.
  Cost if wrong: manual bookkeeping can omit evidence.
- Keep firmware/USB reliability unqualified beyond the bounded read-only pass;
  intermittent failures remain visible and getter retries were not expanded.
  Cost: a later read can still fault and need operator diagnosis.
- Keep physical emission/setters and `6722-P` control limits unqualified because
  they were not authorized/reviewed. Cost: control remains restricted pending
  separate qualification.
- Keep native GUI visual qualification pending because computer-use could not
  initialize; static panel and native data-path tests stand. Cost: a rendering
  or usability defect may remain for operator review.
- Keep private runtime, fresh-machine installation, vendor redistribution and
  automatic Newport/CH340/CP2102 installation separate from this development
  qualification. Cost: first launch selects VISA and missing vendor drivers
  still need their official installation.
