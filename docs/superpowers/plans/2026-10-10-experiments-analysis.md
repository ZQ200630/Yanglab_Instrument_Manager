# Native Waveform, Experiments and Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for coordinated inline implementation; independent components may be delegated under the active multi-agent instructions. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the old oscilloscope reader's usable operations in the native App, with reusable waveform analysis and template-driven multi-instrument experiments.

**Architecture:** Native per-model family drivers feed immutable waveform records. Rust data/analysis and experiment libraries are independent of Tauri and transport; Host services adapt them to the existing authenticated lifecycle, journal and UI. Existing OSA and other drivers remain compatible.

**Tech Stack:** Rust 2021 / workspace MSRV 1.88, existing VISA ABI, serde, serde_json, ring, pinned ZIP/deflate support where required; Tauri 2 with existing HTML/CSS/ES modules. No Python.

**Spec:** `docs/superpowers/specs/2026-10-10-experiments-analysis-design.md`.

## Global Constraints

- Preserve all pre-existing dirty changes and legacy experiment files; work on `codex/scope-workbench`, no automatic merge/push.
- Real-only product; no hardware enumeration, connection or output changes during offline implementation.
- New driver code in `Code/Utils`; native diagnostics in `Code/Debugs`; native experiment library/cases in `Code/Experiments`; data library in `Code/Data`; generated evidence in `Result/experiment-upgrade-20261010`.
- Read-only connect/normal close preserve scope/generator panel/output state. No *RST, *CLS, autoset, automatic output-off on opening a page or automatic RUN after reading existing data.
- No JSON transport of full deep-memory arrays. Preserve actual sample dtype, per-channel axes and raw evidence.
- English light UI, simple normal settings, collapsed advanced fields, immediate pending feedback and truthful outcomes.
- Use existing authority and journals for manual/automated actions. Cancellation and restart never replay unknown commands.

## Review Focus

- Partial binary transfer or dtype conflict must fail/preserve evidence, never yield a plausible corrupt waveform.
- Multi-role preflight failure must issue no actuation; a remote or missing role must not be silently ignored.
- Switching frame/ROI while analysis runs must not attach a stale result to a new selection.
- Duplicate record names and hostile archive members must not overwrite or escape the chosen destination.
- Device readback/refresh/page construction must not cause output writes; deep-memory display must stay bounded and responsive.

---

### Task 1: Native waveform domain and analysis contracts

**Files:** Create `Code/Data/Cargo.toml`, `Code/Data/src/{lib,waveform,analysis,reduce}.rs`, focused integration tests. Modify root `Cargo.toml`/`Cargo.lock` only in controller session.

**Interfaces:** Produces `Samples`, `Calibration`, `ChannelWaveform`, `WaveformFrame`, `WaveformRecord`, validated ROI/calibrated iterators, bounded viewport envelope, `analyze(&WaveformRecord, &AnalysisRequest) -> Result<AnalysisResult, DataError>`. Controller and implementer freeze serialized structs before Task 2/4/5 consumers.

- [x] Write tests for actual U8/U16 decoding semantics, per-channel time axes, absolute ROI index, min/max original indices and tail inclusion; run to observe RED.
- [x] Implement types/validation and viewport reduction without transport or UI dependencies; confirm GREEN.
- [x] Write known positive-width Lorentzian/Q and known-spacing FSR fixture tests plus flat/short/nonfinite/zero-calibration/ROI errors; observe RED.
- [x] Implement versioned pure numerical algorithms and structured result provenance; compare expected numerical tolerance, confirm GREEN.
- [x] Run `cargo test -p yang-data --offline` and record output; commit only task-owned changes when an isolated task commit is practical.

### Task 2: Rust scope and generator drivers

**Files:** Create `Code/Utils/src/{scope,generator}/...`, common SCPI helpers as justified; modify `Code/Utils/src/lib.rs`, crate manifest and tests. Add native diagnostic adapters under `Code/Debugs` in a later controller integration step.

**Interfaces:** Produces `ScopeModel`, `Scope`, scope options/capabilities and typed capture request; `GeneratorModel`, `Generator`, signal/options/capabilities. All constructors are metadata-only. Devices consume an existing `VisaManager` plus resource and clock; test construction may inject a bounded `ByteTransport`. Family results use Task 1 waveform types; implement `DriverLifecycle` with truthful cleanup receipts.

- [x] Write exact transport transcript tests for DS7054 BYTE and MHO5054 WORD/preamble/block acquisition, fragmented headers/payloads, truncation, cancellation and release; observe RED.
- [x] Implement read-only identity connect, channel/depth/status/run-control, chunked waveform reads with per-model scaling and explicit mode effects; confirm GREEN.
- [x] Write generator tests for all three dialects, read-only connect/close, Triangle mapping, output readback, frequency/Vpp/offset/load limits and combined positive/negative voltage limits; observe RED.
- [x] Implement typed setters and optional phase/load/polarity; reject unsupported combinations before writes, never reset output from constructors; confirm GREEN.
- [x] Consult official programming references for uncertain command/calibration semantics; record supported versus unverified capabilities accurately.
- [x] Run `cargo test -p yang-drivers --offline`; record evidence and source/manual anchors.

### Task 3: Native persistence, compatibility and export

**Files:** Create `Code/Data/src/{store,legacy,npy,npz,csv,export}.rs` and fixture tests. Modify only the data crate manifest for pinned dependencies; controller updates workspace lockfile.

**Interfaces:** Produces `DataStore::{new,list,load,save,rename,remove}`, bounded import entry points, raw round-trip saves and `ExportRequest`/`ExportReport`. Uses Task 1 immutable data and analysis records.

- [x] Test `.db` JSON+NPY fixture, actual unsigned header over conflicting metadata, NPZ/CSV compatibility and persisted analysis reload; observe RED.
- [x] Implement streaming/bounded ZIP/NPY parsing without extraction/pickle, schema validation and import provenance; confirm GREEN.
- [x] Test traversal/absolute paths, duplicate members, invalid shape/type, decompression bounds, duplicate record names, interruption and unequal-channel axes; observe RED.
- [x] Implement atomic commits, collision-safe exports, metadata sidecars and per-file errors; confirm GREEN.
- [ ] Verify legacy sample data from supplied archive without executing legacy Python and run the full `yang-data` suite.

### Task 4: Worker, catalog, protocol and waveform capture integration

**Files:** Modify `App/protocol/src/{wire,limits}.rs`, Host/Worker `catalog.rs`, Worker `actions.rs`, `native_sessions.rs` or focused new family adapter, `session.rs`, `backend.rs`, capture storage, Host validators/archive and `App/catalog/devices.json`.

**Interfaces:** New driver kinds `scope` and `generator`; typed actions exposed through existing prepare/execute. Add `waveform` capture kind with checked byte/sample metadata and the existing chunk-fetch lifecycle. OSA descriptors retain unchanged validation. Model IDs `ds7054`, `mho5054`, `dg4162`, `dg4202`, `dg922pro`.

- [x] Test all model/profile admissions including USB/GPIB/TCPIP canonical resource reservations and unknown identity rejection; observe RED.
- [x] Wire family sessions, capabilities and cached observation with no sample arrays; pass action and lifecycle tests.
- [x] Test waveform capture origin, chunk/recovery/close behavior and preservation of OSA invalid-descriptor rejection; observe RED.
- [x] Generalize captures by explicit kind, spool immutable waveform payload and enforce separate declared limits; confirm GREEN.
- [x] Add native debug preview/read-only commands and tests; do not execute hardware stages.
- [x] Run native Worker/protocol/Host regression and record evidence.

### Task 5: Experiment templates and runner

**Files:** Create `Code/Experiments/Cargo.toml`, `src/{lib,model,runner,templates}.rs`, cases `q_scan`, `fsr_scan`; preserve existing Python files. Add focused tests and Host experiment adapter.

**Interfaces:** Produces `ExperimentDefinition`, `RoleBinding`, `RunRecord`, `RunStatus`, `ExperimentPort`, template preflight and runner with confirmed cancellation. Port uses typed family operations routed by Host/domain; it never opens transports.

- [x] Test required role/capability/parameter validation, deterministic ownership acquisition and no writes on partial preflight failure; observe RED.
- [x] Implement waveform capture, Q and FSR templates with versioned parameter snapshots; confirm GREEN.
- [x] Test normal run steps, coarse-tune readiness, cancellation, uncertain command, timeout, disconnect and partial persisted outcomes; observe RED.
- [x] Implement bounded Host-owned runner, shared manual ownership, per-device cleanup and no automatic command replay; confirm GREEN.
- [x] Add authenticated remote role handling through existing client machinery or an explicit pre-actuation rejection until that adapter is implemented; this is a remaining requirement, not silent success.
- [x] Run `cargo test -p yang-experiments --offline` plus Host adapter tests.

### Task 6: Host data and analysis services

**Files:** Create focused Host workbench/job modules; modify Host service dispatch plus GUI/remote allowlists/routing and `App/web/{host-client,remote-client}.js` only where needed.

**Interfaces:** RPCs `data_list`, `data_get`, `data_viewport`, `data_update`, `data_remove`, `data_import`, `data_export`, `analysis_start`, `analysis_status`, `job_cancel`, `experiment_templates`, `experiment_save`, `experiment_list`, `experiment_start`, `experiment_status`, `experiment_stop`. Start replies carry bounded job IDs; status carries stage, measured progress and results/error. File pickers remain native local GUI affordances, not arbitrary remote path authority.

- [x] Test admission/limits, job queue saturation, cancelled imports, local/remote identity and results-channel availability during slow work; observe RED.
- [x] Implement bounded native jobs/store/analysis and owner-scoped experiment adapters with small reply/view limits; confirm GREEN.
- [x] Test recovery/history on Host restart and stale job/result IDs without command replay.
- [x] Run Host/GUI/remote suites and record test evidence.

### Task 7: Direct instrument pages and Analysis workbench

**Files:** Create `App/web/{scope,generator,analysis,data-browser,waveform-plot}.js`, focused CSS and JS tests; modify navigation, model rendering, actions and driver icons while preserving current UX fixes.

**Interfaces:** Consumes Tasks 4/6 APIs. Scope: Screen/Memory, channel enable versus capture/display selection, frames/depth, Collect/Cancel, actual progress; generator: per-channel signal/readback/explicit output commands and optional advanced operations. Analysis: Record/Frame/channel browser, edit/save/load/delete/export, viewport ROI/reset, Q/FSR settings/overlays and results.

- [x] Write UI/action tests for no writes on render, compatible choices, bounded plot data, stale-result rejection and honest pending/failed/partial states; observe RED.
- [x] Implement English light pages matching legacy operations without legacy hidden writes; confirm GREEN.
- [x] Test history/analysis without instrument connection, append/load/export-all and preserved selection/focus during jobs.
- [x] Run all frontend tests and render actual pages for visual/interaction inspection.

### Task 8: Experiments UI and end-to-end integration

**Files:** Create `App/web/experiments.js` and tests; wire navigation/RPC actions/run history and Analyze links; update setup/overview for new real models.

**Interfaces:** Consumes template roles and capabilities, saved experiment definitions and RunRecords; selects explicit local/remote devices/channels. Normal controls show only required decisions; advanced configuration is collapsed.

- [x] Test create/edit/bind/preflight/run/stop/results navigation with real native task boundaries and injected offline transport only in tests; observe RED.
- [x] Implement templates, role selectors, parameter editor, run progress/history and result-to-analysis linking; confirm GREEN.
- [x] Exercise delayed acquisition, UI navigation during processing, contention, role loss and cancellation without false success.
- [x] Verify an end-to-end scope capture -> durable record -> viewport -> Q/FSR -> export flow plus experiment runner -> same durable record flow.

### Task 9: Full verification, documentation and portable delivery

**Files:** Update relevant native development/driver docs and AGENTS extension rules; generate logs/screenshots/package under `Result/experiment-upgrade-20261010`.

- [x] Run `App/scripts/test-native.ps1 -Offline` with installed native Cargo/Node and an explicit writable target directory; inspect failures, do not hide baseline defects.
- [x] Perform one fresh whole-change review of boundaries, scientific corrections, ownership, compatibility and UX; fix important defects with failing regression tests.
- [x] Build the portable native App, verify payload hashes and Python-free packaging, and render the new pages without hardware connections.
- [x] Audit every spec requirement against code, test/runtime evidence and delivered artifact. Distinguish implemented/offline-verified/hardware-unverified; keep the goal active for any unfinished requirement.
- [x] Leave a concise handoff with runnable package, evidence, known limitations and staged hardware diagnostics. No merge/push without a new request.

## Final qualification

Completed offline implementation on `codex/scope-workbench` without hardware, Python, installation, commits or push. Final review corrected Screen freshness in the shared Host path; all Collect/template sources now require confirmed fresh single acquisitions. The actual stopped-Screen two-frame regression observed RED→GREEN; the complete corrected run passed 853 Rust tests, 567 frontend tests and 13 packaging cases. NSIS and closed 29-file portable delivery are in `Result/experiment-upgrade-20261010/package-final`, source `tree-11d20a2d6cfb648813d9142be5e8db6d4602779ff991931166a46975fd5d0726`.

The single unchecked legacy-data step is unavailable external-input qualification: supplied source was inspected, but no measured legacy dataset was available; native compatibility tests use hand-authored format fixtures. This is not a claim of measured-data or bitwise SciPy equivalence. Physical instruments, native desktop clicks and a clean no-Python Windows installation remain unverified, as recorded in the independent requirement audit. Native desktop automation failed initialization; no alternate input workaround was used. Mixed-Host experiment roles explicitly reject, and remote import/automation remain outside the current historical-data authorization.

Evidence and handoff: `Result/experiment-upgrade-20261010/{final-native-corrected.log,package-final-build.log,requirements-audit.md,HANDOFF.md,progress.md}`. Production-page screenshots use finite boundary doubles, not hardware. The original goal's implementable offline scope is complete; physical qualification remains a separate staged workflow.
