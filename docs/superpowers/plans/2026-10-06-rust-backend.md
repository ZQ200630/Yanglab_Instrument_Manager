# Rust Instrument Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Native inline execution is recommended for efficiency; implementation starts after the operator reviews this plan and confirms the execution method.

**Goal:** Replace the entire Python instrument backend with Rust, preserving device safety, data, remote operation and responsive interaction, and deliver a Windows installation that needs no Python or Anaconda.

**Architecture:** Keep the existing frontend, Rust GUI bridge and Rust Host. One independent native worker owns all physical sessions and composes reusable Rust drivers and the Fiber Setup; the Host retains leases, remote access, durable archives and supervision. Migrate common contracts first, then OSA end-to-end, then the remaining drivers, and finally the production package without a Python fallback.

**Tech Stack:** Rust edition 2021, minimum Rust 1.88, Cargo workspace, existing locked serde/serde_json/ring/windows-sys dependencies, installed Windows x64 VISA C ABI, Windows COM I/O, existing Tauri 2/Tokio/rustls Host, Node built-in frontend tests, PowerShell build scripts and NSIS packaging. No new transport framework, Python build helper or shared-memory protocol.

**Spec:** [Rust backend design](../specs/2026-10-06-rust-backend-design.md), approved 2026-10-06. Read the spec and the active `AGENTS.md` before implementation.

## Global Constraints

- Work on `codex/pic-desktop`; no main merge, other machine-branch rewrite or GitHub publication is included.
- Final production launch, native diagnostics, backend regression and installation have no Python, Anaconda, pip or private CPython dependency. Legacy source remains recoverable and is not shipped.
- Drivers stay in `Code/Utils`, setups in `Code/Setups`, diagnostics in `Code/Debugs`, future experiments in `Code/Experiments/<name>`, and outputs in `Result/<name>`. `reference_code` is read-only.
- Use one root Cargo workspace and lockfile. Generated output never goes in `Code/Utils`; preserve reviewed third-party dependency versions when moving the existing lockfile.
- Preserve worker v3 action/result semantics and public Host/network protocols. Startup identity revision 1 identifies the native worker; no arbitrary frontend-supplied executable or fallback.
- Worker limits: 64 domains, 31 pending normal requests, 226 reply slots, 225 responsibility slots, four ordinary and four observation execution slots, reserved safety/management capacity.
- Worker request limit 65536 UTF-8 bytes, depth at most 32, integer magnitude at most 9007199254740991; keep existing finite reply framing and the five terminal phases. A transport timeout is not a terminal instrument result.
- OSA limits: 200001 points, instrument chunks at most 1024 points and replies at most 65536 bytes. No implicit INIT, ABORt or panel-setting write when reading an existing trace.
- Capture limits: descriptor schema 1, interleaved little-endian f64, at most 32 staged captures and 128 MiB, metadata at most 8192 bytes, delivery chunks at most 16384 bytes. ACK follows durable Host import.
- Voltage: 0–14 V, normal steps at most 0.1 V with at least 50 ms between steps; handled faults/shutdown request immediate all-channel zero.
- Gain: 0–200 mA, target 15–40 degC; enable current only with TEC on and fresh measured target +/-0.2 degC continuously for five seconds. Disable current before TEC.
- PM400: preserve settings on connect/normal close, gate sensor operations before writes, and require explicit confirmation for reset/zero/calibration/response/adapter changes.
- MDT: ceiling 75 V, 0.1 V/50 ms normal ramps, read-only connect/close/recover, fault stop-and-hold. Never automatically zero, roll back or adopt a baseline.
- Fiber: left `2110148249-10`, right `160721175410`; logical +X right, +Y away from operator, +Z up. Left XYZ maps to MDT YXZ, right to XYZ. Toward-chip moves <=0.2 um; other per-axis moves <=1.0 um. Nominal conversion requires authorization; estimates remain session-only.
- Every slow interaction acknowledges immediately, preserves unrelated navigation/drafts/focus/scroll and shows honest waiting/outcomes. Only measured work receives a percentage; timeout never replays a command.
- No actual resource enumeration or instrument access during offline implementation. Enumeration, identity/read-only connection and output-changing diagnostics require separate authorizations.
- Keep resource release, confirmed lifecycle actions and physical output evidence separate. Retain unfinished calls, reservations and cleanup obligations; never kill an active owner for convenience.

## Review Focus

1. **Configuration/installation under spaces, Unicode, denied writes or reparse paths:** preserve original records and trust, block unsafe replacements and keep the App usable; pinned in Tasks 11 and 21.
2. **Canonical aliases, externally owned resources and late native I/O completion:** reject duplicate ownership and retain handles/reservations until actual completion; pinned in Tasks 2–4.
3. **Trace context changing mid-read, binary truncation or disk failure after a read:** reject invalid captures, keep exact samples and do not perform another measurement; pinned in Tasks 7–9 and 12.
4. **Disconnect/revocation/EOF during stalled work or saturated queues:** fence queued actions, retain uncertain attempts and keep another domain/remote observer responsive; pinned in Tasks 5, 10, 12 and 19.
5. **Developer fixture, retired source launch or Python payload leaking into production:** tests remain finite offline boundaries, production accepts only native launch and packaging rejects legacy payloads; pinned in Tasks 10, 20 and 21.

## File boundaries and migration checkpoints

| Boundary | New or modified files | Responsibility |
| --- | --- | --- |
| Workspace | root `Cargo.toml`, `Cargo.lock`, `.gitignore`; existing `App/src-tauri/Cargo.toml` | Shared dependency graph; generated output excluded |
| Pure protocol | `App/protocol/src/{lib,wire,identity,limits}.rs` | Strict worker messages, framing and native startup contract; no GUI/hardware |
| Drivers | `Code/Utils/src/{lib,error,clock,lifecycle}.rs`, `transport/`, `osa/`, `voltage/`, `gain/`, `pm400/`, `mdt/` | Typed device libraries, transport ownership and safety |
| Setup | `Code/Setups/src/{lib,fiber,calibration}.rs` | Laboratory mapping and motion authority |
| Worker | `App/worker-rs/src/{lib,main,domains,scheduler,safety,verification,discovery,backend,dispatch,observations,captures}.rs` | Bounded native execution, staged verification, monitor/status serialization |
| Supervisor | existing `App/src-tauri/src/{runtime,startup_handshake,worker_root,worker,gui}.rs` and focused `native_worker.rs` | Executable resolution, process identity, native launch and retained responsibility |
| Host/config | existing `host/{service,contracts,registry,configuration,verification,checks,archive,instance}.rs` | Preserve owning-Host behavior and storage; typed migrations |
| UI | existing `App/web/{api,setup,console-ui,panels,main,activity,osa}.js` | Remove interpreter settings; preserve existing device workflows |
| Diagnostics/package | native `Code/Debugs` crate, `App/scripts`, `tauri.conf.json`, `docs/development.md`, `AGENTS.md`, new native CI | Offline/real diagnostics, portable builds and clean-machine qualification |

Create each crate manifest when its first tested deliverable is introduced; do not add empty workspace members. `yang-protocol` has no driver dependency; `yang-drivers` has no Tauri/protocol dependency; `yang-setups` depends on drivers; `yang-worker` depends on all three. Existing Host imports the protocol, not physical drivers. Native diagnostics may exercise driver libraries only after taking the equivalent machine owner guard.

### Shared interface conventions

Use `DriverResult<T> = Result<T, DriverError>`. Constructors validate configuration without I/O and return `DriverResult<Self>`; device `connect(&mut self)`/`probe_identity(&mut self)` return `DriverResult<ProbeReport>`, and `close(&mut self)` returns `DriverResult<CleanupReport>`. A serial-device probe is a distinct driver-owned read-only lifecycle, not its normal safety-writing connect. Resource-manager clones share a lease; dropping one owner cannot close another owner's handles. `ResourceBook` is a cloneable shared reservation service, not a copied independent map.

For existing public operations not expanded into full signatures below, the first parity-matrix step freezes one exact Rust method signature and return type per current method, including inherited/property-generated facades. Physical scalar reads return `DriverResult<f64>`; masks, counts and enum-valued reads retain their exact integer/enum type, not f64. Enable-state reads return `DriverResult<bool>`, status reads return the named typed status, confirmed setters return their existing typed readback, and action-only setters return `DriverResult<()>`. Every matrix row names its safety class, wire transcript, Rust function, offline test and physical-validation status. This is a task deliverable before writing that driver, not permission to drop or guess an API.

Task 1 defines `PackageIdentity` and `CaptureDescriptor` wire types. Package identity schema 1 contains package/source revision, worker protocol 3, startup revision 1 and expected worker executable SHA256; Task 21 emits the complete package manifest with GUI/Host/worker hashes. Task 10 defines `ShutdownReceipt` containing immutable cleanup reports, `all_resources_released` and process-lifecycle state. Task 20 defines `DiagnosticPlan`, `DiagnosticError` and `DiagnosticReport`; reports serialize those existing evidence types rather than interpreting process exit as physical safety.

The incremental candidate becomes Rust-only at Task 11. Until then the currently running App is untouched. In the OSA milestone, other drivers are explicitly unavailable before admission until ported; this is a partial candidate, not a complete migration. Never run two backends against one resource or add a user-selectable backend switch.

### Verification and commits

Commands below run from repository root in a Windows MSVC developer shell with Rust/Node configured as documented. `--offline --locked` is the normal mode after the workspace lock has been updated for a new local member; new local members may require a deliberate offline lock refresh, with unchanged third-party entries verified in the diff. A missing crate/tool is reported, not silently downloaded or substituted. Existing PIC cache paths may support local execution but are not copied into portable scripts.

For every task, RED must fail on the named missing/wrong behavior; a missing tool is not a useful RED. GREEN must pass that same test without changing its expectation. Commit only that task's listed files and matrix/evidence updates after GREEN. No full rebuild after a prose-only edit. A commit is not a hardware acceptance claim.

## Milestone 1 Native foundation

### Task 1 Workspace and pure worker contracts

**Files:** Create root `Cargo.toml`, `App/protocol/Cargo.toml`, `App/protocol/src/{lib,wire,identity,limits}.rs`, `App/protocol/tests/contracts.rs`; modify `.gitignore` and `App/src-tauri/Cargo.toml`; move the reviewed `App/src-tauri/Cargo.lock` to root `Cargo.lock`. Reference `App/worker/contracts_v3.py`, `App/tests/fixtures/v3-contracts.json` and current Host validation.

**Interfaces:** Produce `RequestV3`, `OutcomeV3`, `ContextV3`, `DomainRef`, `DomainConfig`, `Phase`, `NativeIdentity`, `PackageIdentity`, `CaptureDescriptor`, `Limits`; `parse_request(bytes: &[u8]) -> Result<RequestV3, ProtocolError>`, `encode_outcome(id: &str, outcome: &OutcomeV3) -> Result<Vec<u8>, ProtocolError>`. Identity fields follow the spec; wire fields retain current names/omission rules. Requests use the exact current method allowlist.

- [ ] Write `vectors_match_current_v3_contract`, `strict_json_bounds`, `terminal_error_consistency` and `native_identity_is_disarmed` with assertions including:

  ```rust
  assert_eq!(Limits::default().max_domains, 64);
  assert_eq!(Limits::default().max_request_bytes, 65536);
  assert!(parse_request(br#"{"v":3,"v":3}"#).is_err());
  // Each checked-in vector's parsed validity and method must match its fixture.
  // Reject depth 33, bool epochs, oversized integers and non-finite numbers.
  ```

- [ ] Establish the minimum compilable protocol crate/workspace and run `cargo test --offline --locked -p yang-protocol --test contracts`; expect the named assertions to fail before implementation.
- [ ] Implement strict unique-key JSON, bounded UTF-8 line reading, typed context/method validation and exact five-phase serialization. Reuse locked serde; do not validate after duplicate keys have been discarded. Retain the Host's existing 17 MiB outer reply guard rather than conflating it with the 64 KiB request limit.
- [ ] Run the same test plus `cargo metadata --offline --locked --no-deps`; expect all vectors/negative tests pass and an acyclic workspace graph. Confirm no third-party version change in `Cargo.lock`.
- [ ] Commit as `feat: establish native worker contracts and workspace`.

### Task 2 VISA ownership and native ABI

**Files:** Create `Code/Utils/Cargo.toml`, `Code/Utils/src/{lib,error}.rs`, `Code/Utils/src/transport/{mod,visa,visa_abi,reservations}.rs`, `Code/Utils/tests/visa.rs`. References: `Code/Utils/visa.py`, PM/OSA lifecycle tests and installed reviewed `visa.h`/`visatype.h` (read-only).

**Interfaces:** Produce `DriverError`, `DriverResult<T>`, `Deadline`, `CanonicalResource`, `ResourceBook`, `VisaManager`, `VisaSession`, `CloseReport`. `VisaManager::enumerate(&self) -> DriverResult<Vec<String>>`, `canonicalize(&self, name: &str) -> DriverResult<CanonicalResource>`, `open(&self, resource: CanonicalResource, deadline: Deadline) -> DriverResult<VisaSession>`; session `write_all`, bounded `read`, `close`, and `has_responsibility`. Isolate function pointers/unsafe code from the safe API.

- [ ] Write `alias_is_one_resource`, `different_resources_remain_independent`, `enumeration_opens_no_session`, `close_failure_keeps_manager_alive`, `external_busy_is_not_stolen`, `missing_visa_is_dependency_unavailable`; assert fake ABI `open_calls == 0` after enumeration and reservation survives an unconfirmed close.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test visa`; expect RED on reservation/close behavior with an injected finite ABI table, not the installed library.
- [ ] Implement absolute system-path DLL resolution and reviewed x64 C types, canonical alias parsing, per-session transaction serialization and manager/module leases. Preserve VISA status/count evidence; reject count overflow and partial writes not completed by the deadline. Do not unload a module or release a manager with live calls/handles. Add only required features to locked windows-sys.
- [ ] Run the same test; expect no real DLL loading/session open in tests. Inspect the ABI wrapper against vendor headers and record reviewed signatures in module documentation.
- [ ] Commit as `feat: add bounded native VISA transport and leases`.

### Task 3 Serial I/O and discovery

**Files:** Create `Code/Utils/src/transport/{serial,serial_discovery,serial_abi}.rs`, `Code/Utils/tests/serial.rs`; extend shared reservation module. References: existing serial driver protocol framing and `App/worker/discovery.py`.

**Interfaces:** Produce `SerialConfig`, `SerialDeviceInfo`, `SerialSession`; `enumerate_serial() -> DriverResult<Vec<SerialDeviceInfo>>`, `SerialSession::open(config: SerialConfig, book: &ResourceBook) -> DriverResult<Self>`. `ByteTransport: Send` supplies `write_all(&mut self, bytes: &[u8], deadline: Deadline) -> DriverResult<()>`, `read_bounded(&mut self, maximum: usize, deadline: Deadline) -> DriverResult<Vec<u8>>`, `close(&mut self) -> DriverResult<CloseReport>` and `has_responsibility(&self) -> bool`; implement it for serial and VISA without exposing raw calls in experiment APIs.

- [ ] Write `com_alias_is_one_resource`, `usb_serial_not_port_order`, `partial_io_preserves_frames`, `enumeration_opens_no_port`, `cancel_request_does_not_confirm_release`; assert `COM12` and `\\.\COM12` reserve one port, VID/PID/serial survive enumeration and buffers remain alive while completion is pending.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test serial`; expect RED using finite injected Windows I/O responses.
- [ ] Implement native discovery without opening ports, exclusive COM opens, DCB/timeouts and overlapped operations whose cancellation/completion is settled explicitly. Validate all lengths/whole-operation deadlines; separate command framing from continuous telemetry. Do not infer instrument identity from CH340/CP210x alone.
- [ ] Run the same test; expect no real COM access and correct short-read/late-completion outcomes. Test all three reviewed protocols at their configured 115200 baud without touching an adapter.
- [ ] Commit as `feat: add native serial transport and identity discovery`.

### Task 4 Shared lifecycle, clock and immutable evidence

**Files:** Create `Code/Utils/src/{clock,lifecycle}.rs`, `Code/Utils/src/transport/owner.rs`, `Code/Utils/tests/lifecycle.rs`; create `App/worker-rs/Cargo.toml`, `App/worker-rs/src/{lib,domains}.rs`, `App/worker-rs/tests/domains.rs`. Reference current domain/session/resource/evidence modules and Host instance guard.

**Interfaces:** Produce trait `Clock: Send + Sync` with `now(&self) -> Duration` and `wait(&self, duration: Duration)`, `DriverState`, `CleanupReport`, `ProbeReport`, `OwnerGuard`, `DomainRegistry`. Registry `configure(config: DomainConfig) -> Result<ContextV3, WorkerError>`, `retire(domain: &DomainRef, rev: u64) -> Result<(), WorkerError>`, `snapshot() -> Vec<DomainSnapshot>`; define `DomainSnapshot` with context/config revision/state/responsibility. `DriverLifecycle` exposes `close() -> DriverResult<CleanupReport>` and `has_responsibility() -> bool`. Define `WorkerError` here for worker adapters; define transport errors in Task 2 rather than coupling drivers to protocol errors.

- [ ] Write `old_generation_cannot_publish`, `retired_domain_cannot_resurrect`, `cleanup_attempt_is_immutable`, `pending_close_blocks_reopen`, `diagnostic_owner_guard_conflicts_with_host`; assert newer success does not mutate an older report and a failed close retains its reservation.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test lifecycle` and `cargo test --offline --locked -p yang-worker --test domains`; expect the named behavioral failures. Owner-guard tests use a unique test name; the production guard remains `Global\YangLabInstrumentHost` with existing user security.
- [ ] Implement generation fencing, responsibility retention and explicit retry reports. `Drop` releases ordinary memory but does not fabricate hardware cleanup. Retain failed-to-release native objects in an owned responsibility registry until completion; do not lose them when a session wrapper drops. Add a fake monotonic clock for tests.
- [ ] Repeat both commands; expect all pass without sleeping five real seconds or sharing the live machine guard.
- [ ] Commit as `feat: preserve native lifecycle responsibility and evidence`.

### Task 5 Scheduler and retained safety work

**Files:** Create `App/worker-rs/src/{scheduler,safety,observations}.rs`, `App/worker-rs/tests/{scheduler,observations}.rs`. References: `App/worker/{scheduler,domains,safety,observations}.py` and corresponding regression tests.

**Interfaces:** Produce `Scheduler::new(backend: Arc<dyn Backend>, clock: Arc<dyn Clock>, limits: Limits) -> Result<Self, WorkerError>`, `submit(request: RequestV3) -> Result<PendingOutcome, WorkerError>`, `snapshot() -> SchedulerSnapshot`, `begin_shutdown()`, `join_when_released(deadline: Deadline) -> Result<(), WorkerError>`; a deadline error retains responsibility, not release. `PendingOutcome` retains request/context/phase until delivered. Define `SchedulerSnapshot` with current domains, pending/active counts and responsibility state, and `Backend: Send + Sync` with `execute(&self, request: &RequestV3) -> OutcomeV3` and `observe(&self, context: &ContextV3) -> Observation`; define `Observation` freshness/revision/unit/evidence fields from current schema.

- [ ] Write `bounded_admission`, `safety_fences_queued_normal_work`, `late_completion_keeps_original_context`, `stalled_domain_does_not_block_status`, `readback_failure_has_distinct_phase`, `observer_callbacks_cannot_lose_reply`; assert limits 31/226/225 and 64 domains, safety admission after ordinary saturation, and no reply/action replay after a deadline.
- [ ] Run `cargo test --offline --locked -p yang-worker --test scheduler` and `--test observations`; expect RED with latch-controlled finite backend calls, not arbitrary long sleeps.
- [ ] Port the current fairness/context/phase model with four ordinary and four observation slots, reserved management and bounded safety capacity. All callbacks/native calls run outside scheduler locks. Serialize I/O at the driver transaction boundary; safety priority cannot interleave bytes into an unfinished transaction. Retain active work until actual completion.
- [ ] Repeat both commands; expect independent status and unrelated-domain work succeed while one call is held, and shutdown remains pending rather than claiming release.
- [ ] Commit as `feat: port bounded worker scheduling and safety fencing`.

### Task 6 Catalog admission, enumeration and staged verification

**Files:** Create `App/worker-rs/src/{discovery,verification,catalog}.rs`, `App/worker-rs/tests/{verification,discovery}.rs`. Reuse `App/catalog/*.json`; reference current Python catalog/configuration/verification/resource code and existing Rust Host catalog.

**Interfaces:** Produce `Inventory`, `VerificationProof`, `ProbeAuthorization` and `ProbePort: Send + Sync` with `probe_readonly(&self, config: &DomainConfig, authorization: &ProbeAuthorization) -> Result<ProbeReport, WorkerError>`. `Discovery::inventory() -> Inventory`, `Verifier::new(port: Arc<dyn ProbePort>, clock: Arc<dyn Clock>)`, `Verifier::probe(config: &DomainConfig, authorization: &ProbeAuthorization) -> Result<VerificationProof, WorkerError>`, `register_verified(proof_id: &str, digest: &str, rev: u64) -> Result<(), WorkerError>`. Evidence binds actual identity, configuration digest/revision, stage and session; driver-specific probe implementation is added with each driver.

- [ ] Write `catalog_rejects_model_transport_mismatch`, `missing_visa_does_not_hide_serial_inventory`, `only_current_bound_proof_registers`, `readonly_stage_cannot_normal_connect_gain_or_voltage`, `setup_requires_registered_distinct_serials`; assert forged/expired/rebound proof is rejected before a factory opens hardware.
- [ ] Run `cargo test --offline --locked -p yang-worker --test verification` and `--test discovery`; expect RED with injected inventory/probe ports.
- [ ] Implement existing typed profile allowlists and canonical resource conflicts, including setup/member ownership. Enumeration produces availability, not verified device identity. Periodic checks use authorized staged probe policy and skip/defer an already active incompatible transaction; no startup output-affecting connect.
- [ ] Repeat both commands; expect zero normal-connect calls in identity-only tests and no promotion of saved unverified/synthetic records to authority.
- [ ] Commit as `feat: port native resource admission and staged verification`.

## Milestone 2 OSA vertical migration

### Task 7 OSA trace model and bounded decoding

**Files:** Create `Code/Utils/src/osa/{mod,trace,decode,context}.rs`, `Code/Utils/tests/osa_trace.rs`, `Code/Utils/tests/fixtures/osa/`, `docs/development/rust-driver-parity.md`. References: `Code/Utils/osa_trace.py`, `Code/Debugs/test_osa_trace.py`, current OSA context validation.

**Interfaces:** Produce `TraceId` A–G, `TransferFormat` ASCII/REAL32/REAL64, `TraceContext`, `TraceCapture`, `NativeUnit` dBm/W, `Spectrum`; `decode_trace_reply(reply: &[u8], format: TransferFormat, expected: usize) -> DriverResult<Vec<f64>>`; `TraceCapture` exposes immutable wavelengths/native samples/context/timing/consistency. Capture construction rejects mismatched lengths/non-finite values; no lossy GUI representation replaces archived samples.

- [ ] Map every current OSA public method/property, state, wire command and negative case to its Rust target and test in the parity matrix. Include explicit acquisition/sweep methods, resource-responsibility/probe/close and metadata—not only `read_trace`. Freeze finite ASCII/binary/context fixtures from reviewed protocol/tests without running Python as an oracle.
- [ ] Write `ascii_real32_real64_exact_values`, `truncated_block_is_rejected`, `nonfinite_and_density_are_rejected`, `trace_limits_are_exact`; assert 200001 is accepted as total points, 200002 rejected, 1024-point chunks permitted, 65537-byte replies rejected, and REAL32 widening preserves the exact native f32 value.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test osa_trace`; expect named decoding/bound failures.
- [ ] Implement strict definite-length binary framing, finite ASCII grammar, reviewed wavelength conversion and unit/context validation. Keep unsupported density/CALC/frequency interpretations explicit. Do not change instrument transfer format to simplify decoding.
- [ ] Repeat the command; expect exact fixture samples/metadata pass. Commit as `feat: port exact native OSA trace decoding`.

### Task 8 OSA session, existing-trace read and owned sweeps

**Files:** Create `Code/Utils/src/osa/{session,read,sweep}.rs`, `Code/Utils/tests/{osa_read,osa_lifecycle}.rs`; extend OSA parity matrix. Reference `Code/Utils/osa.py` and existing read/lifecycle diagnostics tests.

**Interfaces:** `Osa::new(manager: VisaManager, resource: String, clock: Arc<dyn Clock>) -> DriverResult<Self>`; `connect(&mut self) -> DriverResult<ProbeReport>`, `probe_identity(&mut self) -> DriverResult<ProbeReport>`, `read_trace(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<TraceCapture>`, `acquire_trace(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<TraceCapture>`, `acquire(&mut self, trace: TraceId, deadline: Deadline) -> DriverResult<Spectrum>`, `close(&mut self) -> DriverResult<CleanupReport>`. `read_trace` does not start/own a sweep; explicit acquisition may own only the sweep it initiated.

- [ ] Write `read_trace_never_changes_panel`, `changed_context_rejects_capture`, `unchanged_context_is_unproven`, `close_never_aborts_panel_sweep`, `owned_sweep_timeout_retains_responsibility`, `partial_close_can_retry`; assert the read transcript contains no INIT/ABORt/settings writes, all ranges <=1024 and before/after metadata is compared.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test osa_read` and `--test osa_lifecycle`; expect RED with strict finite VISA transcripts.
- [ ] Implement identity gating, actual query order/deadlines, complete existing public API and generation-safe release. Make read/decode timings separate from later disk timings. Bound explicit sweep polling; timeout/cancel retains correct sweep ownership and never aborts a panel-owned sweep.
- [ ] Repeat both commands; expect changed context discards the capture, unchanged context remains `unproven`, and all OSA matrix rows have a passing Rust test. No physical test is run here.
- [ ] Commit as `feat: port OSA lifecycle and panel-preserving trace reads`.

### Task 9 Rust capture staging and durable-import compatibility

**Files:** Create `App/worker-rs/src/captures.rs`, `App/worker-rs/tests/captures.rs`; reuse or extract reviewed native file helpers without a driver dependency; add immutable fixture descriptor/payload under `App/worker-rs/tests/fixtures/capture/`. Reference Python staging, current Host archive/runtime import and tests.

**Interfaces:** `CaptureSpool::open(root: PathBuf, nonce: String) -> Result<Self, WorkerError>`, `stage(&mut self, capture: &TraceCapture) -> Result<CaptureDescriptor, WorkerError>`, `read_chunk(&self, nonce: &str, id: &str, offset: u64, length: usize) -> Result<Vec<u8>, WorkerError>`, `ack(&mut self, nonce: &str, id: &str, sha256: &str) -> Result<(), WorkerError>`. `CaptureDescriptor` retains schema 1 and current field names; hash with existing locked ring SHA256. The protocol crate defines descriptor wire types, not filesystem ownership.

- [ ] Write `payload_and_descriptor_match_host_fixture`, `bounds_and_nonce_are_enforced`, `reparse_swap_cannot_redirect_io`, `disk_failure_keeps_capture_unacked`, `ack_requires_exact_hash`, `duplicate_ack_is_idempotent`; assert interleaved LE f64 bytes match exactly and limits 32/128 MiB/8192/16384 are enforced before allocation/admission.
- [ ] Run `cargo test --offline --locked -p yang-worker --test captures`; expect RED using owned temporary directories and injected bounded storage failures.
- [ ] Implement pinned contained Windows directories/files, reparse/identity checks, immutable writes and retained orphan responsibility. Preserve existing Host transfer/ACK semantics; no giant JSON samples, shared memory or fresh hardware read on storage failure.
- [ ] Repeat the command and add the maximum-size fixture import to current Host archive tests; expect sample/hash/metadata equality and ACK only after durable import succeeds.
- [ ] Commit as `feat: port bounded native capture staging`.

### Task 10 Independent Rust worker process and OSA dispatch

**Files:** Create `App/worker-rs/src/{main,backend,dispatch,session}.rs`, `App/worker-rs/tests/{process,dispatch}.rs`; extend protocol identity and verification modules. Reference `Code/Setups/session.py` as well as the current controller lifecycle adapter. Add a diagnostic-only native process fixture in Task 11, never to the production catalog/package.

**Interfaces:** `Worker::new(backend: Arc<dyn Backend>, scheduler: Scheduler, spool: CaptureSpool) -> Self` and `run_io(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Result<ShutdownReceipt, WorkerError>`. `NativeBackend` implements Task 5 `Backend`; `DeviceSession: DriverLifecycle + Send` supplies typed `action(&mut self, name: &str, args: &Value, context: &ContextV3) -> OutcomeV3` and `observe(&mut self, context: &ContextV3) -> Observation`; `DriverFactory::create(&self, config: &DomainConfig) -> Result<Box<dyn DeviceSession>, WorkerError>`. Native `InstrumentSession` retains `request_stop`, `check_health` and retryable `close` responsibilities from the current session adapter; cleanup reports use Task 4 evidence types. Probe routes use the distinct Task 6 probe port, not normal connect.

- [ ] Write `startup_is_native_disarmed_and_empty`, `activation_nonce_is_one_use`, `stdout_is_protocol_only`, `unknown_action_opens_nothing`, `eof_reports_unfinished_cleanup`, `session_retries_retained_release`, `blocked_stdout_keeps_accepted_responsibility`, `osa_action_stages_not_inline_samples`; assert wrong session/epoch/revision fails before I/O, and all non-OSA factories reject as unsupported during this milestone.
- [ ] Run `cargo test --offline --locked -p yang-worker --test process` and `--test dispatch`; expect RED through injected finite library backends and bounded in-memory channels.
- [ ] Implement strict CLI (`--real`, protocol 3 and supervised native paths/nonces only), current method handlers, async reply writer and empty startup. Reject simulate/interpreter/raw-command/fixture arguments before activation. EOF/supervised shutdown stops admission and awaits explicit driver cleanup; log only to stderr. Production `main` constructs only `NativeBackend`, with no environment-selected test transport.
- [ ] Repeat both commands; build `cargo build --offline --locked -p yang-worker --bin yang-worker`. Expect native identity and idle EOF process exit, no enumeration/session calls, and separate software support status for OSA versus unported devices.
- [ ] Commit as `feat: add independent native worker and OSA dispatch`.

### Task 11 Native supervision and configuration migration

**Files:** Modify `App/src-tauri/src/{runtime,startup_handshake,worker_root,gui}.rs`, `App/src-tauri/src/host/{service,contracts,registry,instance}.rs`, `App/src-tauri/src/bin/yang-lab-host.rs`; create `App/src-tauri/src/{native_worker,native_worker_tests}.rs`, native diagnostic crate `Code/Debugs/Cargo.toml` with `src/{lib,main}.rs` and `src/bin/worker-fixture.rs`. Modify existing Python-dependent Rust tests to native fixture launch. References: current process identity/startup records, GUI preferences and registry migrations.

**Interfaces:** Replace `RuntimeConfig.python/root` with `launch: NativeWorkerLaunch`, `catalog_root: PathBuf`, existing mode/protocol/nonce/child-record callback. `NativeWorkerLaunch::resolve(resources: &Path, package: &PackageIdentity) -> Result<Self, RuntimeError>`; private native development resolver is source-build-only, not a frontend input. Keep `WorkerRuntime::spawn`, pending reply/broker and actual PID/creation recording. Registry disk schema and GUI preferences both become version 3, retaining all non-Python fields. The legacy config/wire adapter accepts obsolete Python fields only as ignored migration input, never executable selection; new native settings omit them. Keep public protocol versions unchanged and test the current older-client settings fixtures explicitly.

- [ ] Write `native_image_and_package_must_match`, `no_interpreter_search_or_fallback`, `record_failure_prevents_activation`, `retained_child_blocks_replacement`, `registry_v2_and_preferences_migrate_without_identity_loss`, `corrupt_or_future_config_is_not_overwritten`, `unicode_paths_and_denied_backup_are_truthful`. Native fixture responses cover malformed replies, delayed/late terminals, process exit and unfinished close—not physical data presented to a GUI.
- [ ] Build the fixture with `cargo build --offline --locked -p yang-debug --bin worker-fixture`, then run `cargo test --offline --locked -p sil-instrument-console --features host-bin native_worker -- --test-threads=1`; expect RED with the locally built fixture, never Anaconda. Test-only native launch constructors live behind `cfg(test)` and cannot be registered as production commands. Add/port each affected startup/broker/GUI preference test rather than deleting failing legacy cases.
- [ ] Implement native image/package resolution, packaged-only spawn, strict startup revision, durable child identity before nonce activation, bounded stdio and retained-runtime errors. Back up original configuration bytes before atomic migration; remove only obsolete interpreter fields. Preserve device/setup IDs, revisions, check policies, Host names/data root and DPAPI remote identities. Archive/remote disk schemas remain unchanged. Refuse mismatches/blocked cleanup; no auto-reconnect or baseline restore.
- [ ] Repeat the command and all current runtime/startup/registry tests converted to native fixtures; expect no Python executable launch. Test unknown `--python`/source-worker arguments reject before child creation. Keep release candidates separate from the still-running installation.
- [ ] Commit as `feat: supervise native worker and migrate saved configuration`.

### Task 12 OSA local/remote workflow and responsiveness

**Files:** Modify `App/src-tauri/src/host/{service,archive,results}.rs` only where native contracts differ; create `App/src-tauri/src/native_osa_flow_tests.rs` as a `cfg(test)` unit-test module so it can use the private native fixture resolver; modify `App/web/{api,setup,console-ui,panels,main,activity,osa}.js` and affected `App/tests/{local-startup,console-ui,osa-page,activity,shared-results,connections}.test.mjs`. Reuse existing styles, icons, archive schemas and remote trust flow.

**Interfaces:** Keep public Connect → Read trace → Save and Host operations/events/capture identifiers. Remove Python field/arguments from all active settings/API/startup routes; do not add backend controls. Diagnostics expose measured `instrument_io_ms`, `decode_ms`, `staging_ms` without relabeling aggregate wait as measured subphases. Native Host tests use explicit finite worker fixtures; real OSA remains a later separate diagnostic.

- [ ] Write `native_osa_archive_and_export_match_exactly`, `storage_failure_does_not_replay_read`, `remote_observer_sees_same_capture`, `old_pin_is_never_replaced`, plus mounted UI tests `native_startup_has_no_python_input` and `stalled_read_keeps_drafts_navigation_and_previous_capture`; assert pending feedback is visible before the deferred reply, no invented percentage, focus/scroll stay stable and native samples survive CSV round-trip.
- [ ] Run `cargo test --offline --locked -p sil-instrument-console --features host-bin native_osa_flow -- --test-threads=1` and `node --test App/tests/local-startup.test.mjs App/tests/console-ui.test.mjs App/tests/osa-page.test.mjs App/tests/activity.test.mjs App/tests/shared-results.test.mjs App/tests/connections.test.mjs`; expect RED on native launch/settings removal while preserving existing fixtures. Run native tests serially because production owner-guard cases must not overlap an active Host.
- [ ] Connect worker capture descriptors/chunks to existing durable import, events and remote archive transfer; preserve leases and saved trust. Remove interpreter UI copy; keep Settings grouping and small device icons. Render busy/unknown/failure immediately and yield large decoding; do not claim a language rewrite solved an unmeasured device wait.
- [ ] Run `cargo test --offline --locked -p sil-instrument-console --features host-bin -- --test-threads=1` and `node --test App/tests/*.test.mjs`; expect the complete native/frontend offline suites pass without Python. Record fixture stage timings, not real-hardware performance claims.
- [ ] Commit as `feat: complete native OSA archive and remote App workflow`. The Rust OSA candidate is ready for separately authorized enumeration, known `GPIB0::4::INSTR` identity, then existing-trace read. Sweep triggering remains a separate output-action approval; unrelated USB/network resources remain out of scope.

## Milestone 3 Remaining driver parity

### Task 13 Eight-channel Voltage Source

**Files:** Create `Code/Utils/src/voltage/{mod,codec,monitor,ramp}.rs`, `Code/Utils/tests/{voltage,voltage_shutdown}.rs`; extend driver parity matrix. Reference `Code/Utils/voltage.py` and voltage evidence/shutdown tests.

**Interfaces:** Produce `VoltageConfig`, `VoltageStatus`, `ZeroEvidence`, `ZeroState`; `VoltageSource::new(config: VoltageConfig, book: ResourceBook, clock: Arc<dyn Clock>)`; `connect`, separate `probe_identity`, `read_status(max_age: Duration)`, `wait_for_status(Deadline)`, `wait_for_channel(channel: u8, target_v: f64, tolerance_v: f64, deadline: Deadline)`, `set_channel(channel: u8, voltage: f64)`, `set_all([f64; 8])`, `ramp_to([f64; 8])`, `zero(emergency: bool)`, `close`, responsibility/evidence getters, with the shared typed return conventions. Channel indices remain 1–8, never 0–7 at the public boundary.

- [ ] Map all existing public Voltage methods/state/evidence rows. Write `encode_eight_channels_matches_wire`, `telemetry_resynchronizes_without_false_success`, `ramp_is_at_most_point_one_every_fifty_ms`, `invalid_channel_or_voltage_writes_nothing`, `fault_zeroes_all_channels`, `late_zero_evidence_does_not_mutate_old_report`; assert 14.0 accepted, 14.0001/NaN rejected, stale telemetry unusable and frame scale remains reviewed `BOARD_SCALE_V = 28.0` rather than guessing from the board's advertised ceiling.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test voltage` and `--test voltage_shutdown`; expect RED with fake clock/serial transcripts.
- [ ] Port the exact eight-channel wire codec, continuous reader and telemetry freshness. Normal changes use bounded ramps; startup/fault/shutdown zero follows the existing driver policy and disclosed lifecycle. Preserve distinctions `unknown`, `command_sent`, host-observed `measured_zero`; none claims an independent physical measurement. A writer/reader failure retains cleanup liability.
- [ ] Repeat both commands; expect timing/zero order and all API rows pass without a real serial port.
- [ ] Commit as `feat: port voltage source monitoring and safe ramps`.

### Task 14 Gain Chip Driver and temperature interlock

**Files:** Create `Code/Utils/src/gain/{mod,codec,monitor,interlock,pid}.rs`, `Code/Utils/tests/{gain,gain_interlock}.rs`; extend parity matrix. Reference `Code/Utils/gain.py`, reviewed Gain command reference and existing diagnostic/controller cases.

**Interfaces:** Produce `GainConfig`, `GainStatus`; `GainDriver::new(config: GainConfig, book: ResourceBook, clock: Arc<dyn Clock>)`, separate `probe_identity`; typed reads for temperature/target/TEC/current/current-enable; `set_temperature(f64)`, `set_current(f64)`, `enable_tec`, `disable_tec`, `disable_current`, `wait_stable(Deadline)`, `enable_current`, `ramp_current(target_ma: f64, step_ma: f64, interval: Duration) -> DriverResult<f64>`, `read_pid`, `set_pid(p: f64, i: f64, d: f64)`, `reset_pid`, `clear_integral`, `close`. Preserve existing ramp bounds step <=1.0 mA and interval >=50 ms. API return types match current acknowledged/readback evidence, not unconditional booleans.

- [ ] Map all methods and startup/monitor/shutdown paths. Write `gain_fixed_commands_and_ready_fields_match`, `five_seconds_requires_fresh_continuous_samples`, `tec_off_or_monitor_failure_disables_current`, `shutdown_orders_current_before_tec`, `limits_reject_before_write`, `pid_functions_retain_reviewed_protocol`; assert `STCA150000`/`READY;C=150.000`, 200 mA maximum, target [15,40], ±0.2 degC, five seconds, and existing consecutive-sample maximum gap 1.5 s.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test gain` and `--test gain_interlock`; expect RED using fake clock and strict 115200-baud transcripts.
- [ ] Port exact command framing/reply-field/ACK validation, target/current/PID operations and periodic temperature monitor. Any instability/sample gap/monitor failure invalidates stability authority; current enable rechecks fresh TEC/temperature state. Keep current-off before TEC-off for normal close, handled faults and worker EOF. Unknown readbacks remain unknown rather than copied requested values.
- [ ] Repeat both commands; expect boundary/sample-gap/order cases and all Gain API rows pass. Do not perform the former 60 mA physical test as part of this task.
- [ ] Commit as `feat: port gain driver with temperature watchdog`.

### Task 15 PM400 session and measurement families

**Files:** Create `Code/Utils/src/pm400/{mod,session,sensor,measurement,status}.rs`, `Code/Utils/tests/{pm400_session,pm400_measurement}.rs`; extend parity matrix. Reference `Code/Utils/pm400.py`, its facade/property tables and `Code/Debugs/test_pm400.py`.

**Interfaces:** Produce current typed equivalents `InstrumentInfo`, `SensorInfo`, `SensorCapabilities`, `MeasurementKind`, `Measurement`, `PowerUnit`, `StatusGroup`, `SystemError`; `Pm400::new(manager: VisaManager, resource: String, clock: Arc<dyn Clock>)`, `connect`, `probe_identity`, `identify`, `close`; typed `measure(kind, Deadline)`, `fetch(kind)`, `read(kind, Deadline)`, `configure(kind)`, `initiate`, `abort`, `cancel_measurement`, operation-complete/status reads. Preserve all nine measurement kinds and convenience methods, not just GUI power.

- [ ] Map every current public/facade-generated method including clear-on-read status/error semantics; classify these truthfully rather than calling every query nondestructive. Write `connect_close_preserve_measurement_settings`, `nine_measurement_kinds_keep_units`, `unsupported_sensor_never_receives_command`, `two_visa_resources_are_independent`, `cancel_and_late_measurement_keep_ownership`; assert photodiode/thermopile/pyro/adapter/no-sensor capabilities choose only legal operations.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test pm400_session` and `--test pm400_measurement`; expect RED with finite SCPI transcripts and sensor fixtures.
- [ ] Port measurement/configuration/cancellation/status lifecycle and parsing using shared native VISA. Normal connect/close performs no settings/reset/zero writes; measurement-mode changes occur only through their explicit typed operation. Preserve error classifications, timeout responsibility and exact values/units.
- [ ] Repeat both commands; expect each measurement and session matrix row has a passing capability/lifecycle test.
- [ ] Commit as `feat: port PM400 session and measurement families`.

### Task 16 PM400 settings and authorized maintenance

**Files:** Create `Code/Utils/src/pm400/{sense,input,system,display,calibration}.rs`, `Code/Utils/tests/pm400_settings.rs`; extend existing PM session/status modules and parity matrix.

**Interfaces:** Typed `Pm400::{sense,input,system,display,calibration,status}` facades retain current snake-case operation names and enum/value types. Include averaging, wavelength/beam/loss/range/reference/delta/unit, sensor response, thermopile acceleration/time constant, lowpass, adapter type, beeper/date/time/line frequency, display brightness/contrast, calibration string, status masks/transitions, reset/zero/preset and standard/service-event commands. Sensitive methods take `confirm: bool` exactly as the reviewed driver; no public generic SCPI escape hatch.

- [ ] Write `all_property_table_rows_have_typed_rust_methods`, `maintenance_requires_confirmation_before_write`, `capability_checks_precede_any_write`, `range_selectors_and_readbacks_match`, `date_time_masks_and_adapter_limits_are_strict`; assert unconfirmed reset/zero/response/adapter changes yield zero writes, and nonexistent sensor functions are rejected even when `confirm = true`.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test pm400_settings`; expect RED. Parameterize from a reviewed explicit matrix of current property specifications; do not guess method presence from the GUI or simple text search alone.
- [ ] Implement all matrix rows with typed getters/setters and exact validation/readback/error behavior. Sensitive operations keep operator confirmation, sensor gates and session locks; maintenance failures do not imply reset/zero succeeded.
- [ ] Repeat the command plus the two Task 15 tests; expect all current PM API rows accounted for by tests, with firmware/sensor conditionality recorded separately from physical qualification.
- [ ] Commit as `feat: complete PM400 settings and maintenance parity`.

### Task 17 MDT693B protocol and read-only lifecycle

**Files:** Create `Code/Utils/src/mdt/{mod,protocol,session,monitor}.rs`, `Code/Utils/tests/{mdt_protocol,mdt_lifecycle}.rs`; extend parity matrix. Reference `Code/Utils/mdt693b.py`, reviewed manual and existing tests.

**Interfaces:** Produce `Axis`, `VoltageLimit`, `RotaryMode`, `AxisState`, `MdtStatus`, `MdtConfig`; `Mdt693b::new(config: MdtConfig, book: ResourceBook, clock: Arc<dyn Clock>)`, `connect`, `probe_identity`, `recover`, `close`, `get_supported_commands`, `get_product_information`, `get_serial_number`, `get_axis_voltage`, `get_all_voltages`, `get_axis_state`, responsibility/status getters. Monitor never supplies motion authority by itself.

- [ ] Map protocol/lifecycle/read rows. Write `echo_and_prompts_parse_all_reviewed_forms`, `response_is_bounded_to_4096`, `connect_close_recover_write_no_settings_or_voltage`, `external_voltage_is_truthfully_reported`, `recover_leaves_motion_disarmed`, `monitor_fault_holds_not_zeroes`; assert 75 V project ceiling does not cause a startup limit-setting write or clamp a manually present higher reading.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test mdt_protocol` and `--test mdt_lifecycle`; expect RED with strict 115200-baud transcripts.
- [ ] Port prompt/echo/error/fragment handling and read-only discovery/recovery snapshots. Keep command capability evidence and stop-and-hold policy; external/master/manual motion invalidates authority. Never send zero, arrows, output setters or default restoration on connection or fault.
- [ ] Repeat both commands; expect exact protocol and truthful residual state, with no baseline authority from recovery.
- [ ] Commit as `feat: port MDT read-only lifecycle and protocol`.

### Task 18 MDT complete settings and motion authority

**Files:** Create `Code/Utils/src/mdt/{settings,motion,authority}.rs`, `Code/Utils/tests/{mdt_settings,mdt_motion}.rs`; extend parity matrix. Reference all current public MDT methods, authority evidence and arrow/master-scan regression cases.

**Interfaces:** Preserve typed `set_axis_voltage`, `set_all_voltages`, `adopt_current_axis_baseline`, `ramp_to_zero`, `emergency_zero(confirm: bool)`; master-scan state/voltage, axis minimum/maximum, DAC step, channel selection/increment/decrement, echo/friendly name/display/compatibility/rotary/push-to-adjust and authorized factory-default methods. `BaselineAttestation` holds the current required operator evidence and generation; no default constructor arms motion. Existing method-specific confirmation/authority requirements carry over unchanged.

- [ ] Write `read_only_baseline_cannot_move`, `ramp_steps_and_intervals_are_bounded`, `arrow_and_master_scan_are_capability_gated`, `partial_move_holds_and_invalidates`, `emergency_zero_requires_confirmation_and_full_readback`, `settings_do_not_relax_project_ceiling`; assert 0.1 V/50 ms, 75 V maximum, and a partial/failed zero cannot establish authority.
- [ ] Run `cargo test --offline --locked -p yang-drivers --test mdt_settings` and `--test mdt_motion`; expect RED with fake clock and transcript assertions including absence of rollback/auto-zero.
- [ ] Port every settings/motion matrix row, safety preflight, readback and external-change invalidation. Arrow sequences stay inside the reviewed driver protocol, never an experiment/worker raw endpoint. Explicit zero and operator-attested baseline adoption are the only authority-establishing paths allowed by current rules; revalidate all required state before writing.
- [ ] Repeat both commands and Task 17 tests; expect all current MDT API rows supported or truthfully firmware-gated, without unexplained no-ops.
- [ ] Commit as `feat: complete MDT settings and motion safety parity`.

### Task 19 Fiber Setup and all-device worker integration

**Files:** Create `Code/Setups/Cargo.toml`, `Code/Setups/src/{lib,fiber,calibration}.rs`, `Code/Setups/tests/fiber.rs`; extend `App/worker-rs/src/{backend,dispatch,observations,verification,safety}.rs`, `App/worker-rs/tests/{all_devices,telemetry}.rs` and parity matrix. References: `Code/Setups/{fiber_coupling,session}.py`, reviewed configuration and worker telemetry/safety adapters.

**Interfaces:** Produce `StageSide`, `LogicalAxis`, `Vector3Um`, `StageStatus`, `MoveResult`, `FiberConfig`, `FiberCouplingSetup`; `for_members(serials: &[String], config: FiberConfig)`, `connect`, `available_sides`, `stage(side)`, `adopt_baseline(side, attestation)`, `move_by_um(side, delta: Vector3Um)`, `close`. Worker factory now produces all five device types and Fiber setup; setup/member exclusivity remains one reservation boundary. Exact API returns and calibration fields follow the completed setup parity matrix.

- [ ] Map every public Fiber setup/stage/config-loading method and session-adapter behavior before implementation. Write `serials_define_sides_not_com_order`, `one_side_is_supported`, `logical_mapping_and_toward_chip_signs`, `nominal_conversion_requires_authorization`, `partial_motion_invalidates_without_rollback`, `setup_and_member_cannot_double_own`; assert left XYZ→YXZ/right XYZ→XYZ, left +X/right -X toward, <=0.2 um toward and <=1.0 um other, with no persisted/recovered position authority.
- [ ] Write integration cases `all_catalog_actions_have_typed_dispatch`, `gain_voltage_cleanup_order_on_eof`, `stalled_osa_does_not_block_other_domain`, `telemetry_freshness_units_and_cleanup_are_truthful`, `revoke_fences_pending_actions`; every applicable worker action validates context, identity and args before the driver call.
- [ ] Run `cargo test --offline --locked -p yang-setups --test fiber`, then `cargo test --offline --locked -p yang-worker --test all_devices` and `--test telemetry`; expect RED with finite transport/driver doubles, no output.
- [ ] Implement serial-bound setup, reviewed nominal calibration/motion planning and complete worker adapters. Preserve all library functions even when absent from the GUI; worker allowlists stay catalog-defined, never generic driver-method reflection. Use bounded per-device monitors, retaining each monitor's responsibility until joined; stale/failed telemetry remains labeled. Fence pending safety/conflicting work and hold piezos during cleanup.
- [ ] Repeat all three commands and focused driver suites; expect complete API/worker matrix coverage. Commit as `feat: integrate native drivers and logical fiber setup`.

## Milestone 4 Native deployment and qualification

### Task 20 Native diagnostics and production Python retirement

**Files:** Extend `Code/Debugs/src/main.rs`; create `Code/Debugs/src/{args,enumerate,osa,voltage,gain,pm400,mdt,fiber,remote}.rs`, `Code/Debugs/tests/{stages,remote,legacy_boundary}.rs`; modify `App/src-tauri/src/{lib,worker,gui,runtime}.rs`, `AGENTS.md`, `docs/development.md`, `README.md`, `requirements.txt`, `environment.yml`; create `docs/development/legacy-python.md`, `.github/workflows/native.yml`. Retain `App/worker`, old scripts/tests/experiments as labeled reference, not active build paths. Do not remove historical measurement outputs.

**Interfaces:** Native crate/package name `yang-debug`, main executable `yang-debug.exe`: subcommands `enumerate`, `osa`, `voltage`, `gain`, `pm400`, `mdt`, `fiber`, `remote`; each requires an explicit stage/output directory and typed binding. `parse_args(args: &[String]) -> Result<DiagnosticPlan, DiagnosticError>`, `execute(plan: DiagnosticPlan, authorization: DiagnosticAuthorization) -> Result<DiagnosticReport, DiagnosticError>`; define `DiagnosticAuthorization` with explicit stage/binding/confirmation, never infer action approval from enumeration. Default invocation validates arguments and opens nothing. Remote diagnostic reuses existing authenticated Rust Host/client APIs, not a new trust protocol. Fixture executable remains development-only.

- [ ] Write `unknown_or_output_stage_args_open_nothing`, `readonly_voltage_gain_do_not_start_normal_driver`, `diagnostic_refuses_live_owner`, `remote_archive_exact_bytes_and_release_receipts`, `active_backend_tests_and_builds_need_no_python`; assert device actions require the relevant explicit acknowledgement and legacy experiments are never auto-launched/migrated. Move every remaining Python-based Rust process test to the native fixture, keeping failure coverage.
- [ ] Run `cargo test --offline --locked -p yang-debug --test stages` and `--test remote`; expect RED with finite driver/Host boundaries. Test actual TLS with an empty finite test service, not a real hardware Host. Run process/owner-guard cases sequentially.
- [ ] Implement diagnostic staging and immutable machine-local JSON reports under `Result/<name>`. Remove retired Tauri worker commands/source-spawn/import paths from active registration, not merely the visible UI. Update active rules to Rust/native tests and the Rust Fiber boundary; preserve all numeric safety rules. Label Python environment manifests and instructions legacy-only, retaining current pins for legacy references. Native CI uses Windows MSVC/Rust/Node, builds native fixtures and runs no Python setup/commands; CI tests do not load/open hardware.
- [ ] Run all native/frontend tests without Python on the process PATH and compile diagnostics; expect no dependency on Python fixture/server/assembler. Produce a final parity table with distinct `implemented`, `offline passed`, `physical validated` columns, with absent instruments left unvalidated.
- [ ] Commit as `refactor: retire Python production paths and add native diagnostics`.

### Task 21 Portable builds and Python-free installer

**Files:** Replace active `App/scripts/build-host.ps1`/`check-package.ps1` assumptions; create `App/scripts/{build-native,build-package,test-native}.ps1`, `App/scripts/tests/native.Tests.ps1`, `App/runtime/{native-worker,native-manifest}.schema.json`, `App/src-tauri/windows/hooks.nsh`; modify `App/src-tauri/tauri.conf.json`, docs and `.gitignore`. Keep old CPython assembly tools explicitly historical and outside the new entry points.

**Interfaces:** `build-native.ps1 -SourceRoot <absolute> -Cargo <absolute> -TargetDir <absolute> [-Offline]` builds Host/worker and stages architecture-suffixed sidecars; `build-package.ps1` calls the reviewed installed Tauri CLI and emits candidate installer/manifest into a new output directory. `check-package.ps1 -Root -SourceRoot [-GuiHash -HostHash -WorkerHash]` returns verified hashes/allowed resources/no Python payload. `test-native.ps1` builds diagnostic fixtures and runs tests sequentially, propagating nonzero exits. No script takes a Python argument or hardcodes PIC caches.

- [ ] Write PowerShell native tests `PackageRejectsPythonAndFixtures`, `BuildPreservesEnvironmentAndRefusesExistingOutput`, `UnicodeRootResolvesNativeSidecars`, `MissingVendorDependencyIsNotInstalled`, `UpgradeCannotReplaceActiveOwner`. Assert the package excludes `.py/.pyc`, python executables/DLLs, wheels/pip/env manifests, diagnostic fixtures, source launchers, credentials, results and build caches; test the explicit allowed catalog/config/assets and native binaries.
- [ ] Run `pwsh -NoProfile -File App/scripts/tests/native.Tests.ps1`; expect RED using owned fixture directories, never the installed App. Use built-in PowerShell assertions, not an undeclared Pester/Python dependency.
- [ ] Implement portable tool/path detection and build environment restoration. Build worker, bootstrap Host with sidecars disabled, stage both `yang-lab-host-x86_64-pc-windows-msvc.exe` and `yang-worker-x86_64-pc-windows-msvc.exe`, then build GUI/NSIS; normal build includes both sidecars. Replace Python resource mappings with reviewed data/assets. Generate packaged `native-worker.json` for Task 1 `PackageIdentity` before GUI compilation. Generate separate `qualification.json` after compilation with source/package/protocol/startup revisions and all three executable hashes; never embed that post-build hash report into the binaries being hashed. Both use schema 1; startup trusts the worker identity file, not a checksum of the entire App as a security promise.
- [ ] Implement the NSIS pre-install/upgrade check to refuse live Host/worker ownership without kill/restart or output writes. Detect WebView2/applicable vendor dependencies; do not silently accept vendor licenses/install a VISA backend. Add archived-source/third-party notices; no generated payload in Git.
- [ ] Repeat PowerShell tests, full native/frontend suites and package build/check. Expect build scripts leave the shell environment unchanged and installer contents meet the allowlist. Commit as `build: package native Host and worker without Python`.

### Task 22 Clean Windows, real devices and remote acceptance

**Files:** Create `docs/development/native-acceptance.md`; native qualification reports/captures only in a new `Result/console/native-<run>` directory. Update parity/acceptance status with actual outcomes, not inferred completion. No main merge, push, new branch or experiment is part of this task.

**Interfaces:** Native diagnostics from Task 20, exact installer/hashes from Task 21, existing owning/observer GUI workflows. Reports distinguish installer/dependency/empty-startup, per-device identity/read/action, archive/export, cleanup, App restart, Host restart, loopback and actual two-PC/Tailscale stages.

- [ ] Run final hardware-free regression before any device stage:

  ```powershell
  cargo fmt --all -- --check
  cargo test --offline --locked --workspace --all-targets --features sil-instrument-console/host-bin -- --test-threads=1
  node --test App/tests/*.test.mjs
  pwsh -NoProfile -File App/scripts/tests/native.Tests.ps1
  ```

  Expect all pass, no ignored Python-dependent active cases and no actual device access. Run guard/process tests only after the existing live Host has been ordinarily released; never kill it to run tests.
- [ ] Install the exact candidate on a genuine Windows computer/fresh VM without Python/Anaconda/Rust/Node, as an ordinary user. Verify empty startup, Settings, missing-VISA reporting, native worker child identity and clean close; repeat with spaces/Unicode install/data paths, restart and saved configuration/trust. Merely removing Python from PATH on this development machine is not this acceptance. A target unavailable to the agent is reported as pending and requires operator assistance, not an improvised claim.
- [ ] After separate enumeration authorization, run native enumeration only. After separate read-only authorization, validate each known connected instrument identity through its native driver. Do not probe previously ignored unknown USB/network devices. On `GPIB0::4::INSTR`, separately approve reading an existing trace; confirm queried panel context, sample/archive/CSV equality and normal release. No INIT/ABORt/settings writes in that stage.
- [ ] For Voltage/Gain/MDT/Fiber and sensitive PM operations, disclose startup/shutdown/output effects and obtain separate action authorization before normal connection/action. Use the same staged procedure for every available device; absent devices remain physically unvalidated. Never use the prior 60 mA/1 min or stage-zero approvals as authorization for a new migration trial.
- [ ] Validate two native Apps on one PC, then actual two-PC/Tailscale: approve new trust once; trusted reconnect after App/Host restart requires no approval; remote OSA read synchronizes both Apps and archive bytes; navigate/export during a wait; disconnect/revoke and verify normal driver-controlled cleanup. Preserve existing trust and record loopback versus actual network results separately.
- [ ] Record ordinary GUI release, Host/worker exit and immutable resource cleanup reports; uncertain output/release is not a pass. Commit only acceptance instructions and truthful status as `docs: record native backend qualification`. If clean-machine or device qualification is incomplete, hand off the implemented candidate with those specific remaining stages instead of calling final deployment complete.

## Execution checkpoints and handoff

| Checkpoint | Required deliverable | What is not yet claimed |
| --- | --- | --- |
| Tasks 1–6 | Hardware-free native contracts/transports/lifecycle/scheduler/admission | Product launch cutover or physical validation |
| Tasks 7–12 | Rust-only OSA candidate, local archive/export and remote App regression | Remaining driver runtime parity or clean-machine installation |
| Tasks 13–19 | All five driver libraries and Fiber Setup integrated, complete offline parity matrix | Hardware validation for an absent device |
| Tasks 20–21 | No active Python paths, native diagnostics/CI, verified candidate installer | A genuine no-Python Windows acceptance pass |
| Task 22 | Exact-package clean-machine and authorized device/remote evidence | Main merge/publication or unrelated experiment readiness |

Use focused RED/GREEN checks for each task, full native/frontend checks at Task 12 and before packaging/qualification, and one independent whole-change review at the end of inline execution. Review the shared protocol, unsafe ABI/ownership, preserved safety limits, config/trust migration, exact archive bytes and installer contents. Fix important findings with their own regression test. Do not repeatedly run expensive full suites for unchanged documentation.

Implementation method is not assumed from approval of this written plan's scope. Recommended handoff: confirm the plan, then execute inline with `superpowers:executing-plans`; subagent-driven execution remains available if the operator prefers per-task independent reviews. Routine offline edits/tests need no repeated permission question; hardware stages, dependency installation, live-owner replacement, integration and publication retain their separate authorization boundaries.
