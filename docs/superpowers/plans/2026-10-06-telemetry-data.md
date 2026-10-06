# Host Telemetry Recording Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans inline, task-by-task, with one whole-plan independent review and one tested Important/Critical fix pass. The operator selected inline execution and automatically approved equivalent planning handoffs. Steps use checkbox syntax.

**Goal:** Record genuine driver observations and motion/authority events once on their owning Host, preserve partial/error evidence, and provide simple Record/Stop/history/export on each applicable instrument page.

**Architecture:** A bounded worker outbox captures admitted observations without doing file I/O or changing driver cadence. A dedicated Rust recording actor appends verified events durably before acknowledging them; the Host binds each recording to the current owner/domain/configuration and stops it on authority loss. GUI observers consume the same recording references, never create extra pollers or duplicate archives.

**Tech Stack:** Existing Anaconda VISA Python standard library, Rust/serde/Tokio and existing SHA-256/native-file primitives, vanilla JavaScript. No dependency installation.

**Spec:** docs/superpowers/specs/2026-10-06-real-console-design.md, device pages in section 3 and the remaining telemetry/motion requirements in section 4. The OSA one-shot capture increment through fa9e79e is an unchanged dependency, not a task to redo.

## Global Constraints

- All Python runs in Anaconda VISA. Use the current managed isolated worktree; preserve unrelated .gitignore changes and reference_code. No main merge, GitHub push, installer execution, hardware action, unknown-device probe or Tailscale change in this increment.
- Production is real-only. Finite transport doubles are mandatory offline checks, never an instrument backend, GUI data source or physical-acceptance claim.
- Preserve 65536-byte control frames, existing worker/safety capacity, lifecycle barriers, lease/configuration/connection/epoch fences, and one physical Host owner. No raw serial/SCPI escape hatch.
- Default recording interval is 1000 ms; accepted range is 1000..60000 integer ms. Never speed up the existing 2500 ms observation cadence merely for recording. Command, failure and authority events are not interval-decimated.
- Archive root is the existing selected Result root. Group names use archive::valid_name; IDs are 32 lowercase hexadecimal characters. No driver-directory output, GUI-supplied chunk pathname or restored recording authority after restart.
- Voltage remains 0-14 V, <=0.1 V steps, >=50 ms; Gain remains 0-200 mA/15-40 degC, stable TEC interlock, current-off before TEC-off; piezo remains hold/invalidate on fault, never implicit zero/rollback. Recording stop is not instrument disconnect or physical-zero evidence.
- Recording failure is visible and partial. `revoke_on_failure` defaults false; only an explicitly selected true policy requests the existing lease-revocation cleanup, never direct driver writes. Owner release/loss and Host stop always retain their existing hardware cleanup rules.
- Keep historical provenance and exact native units. Requested voltage, confirmed Gain current setpoint and session-only open-loop position estimate are not independently measured physical outputs or displacement.

## Review Focus

- Repeated cached reads or delayed old-context observations: do not renew freshness, manufacture samples or append data under a new owner/connection.
- Disk full, blocked writer or worker outbox overflow: recording becomes visibly partial; safety remains available; acknowledgement follows durable append only, with a reserved terminal/error path.
- Partial Gain rounds and PM400 front-panel mode changes: retain per-field age/quality and sensor/configuration context; never label a setpoint as measured current or issue implicit CONFigure/INIT/READ setters.
- Owner disconnect races a draining batch or an observer closes: drain only the fenced recording's accepted prefix, close once, never stop another owner's hardware/recording or resume after reconnect.
- Corrupt/truncated/replaced files and large history/export: verify containment, framing, exact lengths/hashes and bounded pages; interrupted data stays readable as partial, never a sealed complete result.

## Files and event contract

- Task 1 creates App/worker/telemetry_schema.py and App/tests/test_telemetry_schema.py: pure strict event codec; no driver imports, resource opening, clocks or file I/O.
- Task 2 creates App/worker/telemetry.py and App/worker/telemetry_values.py plus corresponding tests; narrowly modifies observations.py, scheduler.py, domains.py, controller.py, contracts_v3.py and runtime.rs for bounded private outbox ingress/drain/ack/close and typed PM400 fetch.
- Task 3 creates host/recording.rs and host/recording_files.rs with unit tests; modifies host/mod.rs only to expose them. Native directory/file pinning and hashing are reused or extracted from the existing archive/native_files implementation with its existing tests retained.
- Task 4 creates host/recording_service.rs; narrowly modifies service.rs, host_client.rs, GUI native bindings, channel admission and Host fixtures/tests. Recording storage/monitoring must not expand service.rs with a second monolithic subsystem.
- Task 5 creates App/web/recording.js and tests; modifies the actual v3 console-ui.js/main.js/host-client.js/instance-view.js integration, native export bindings, README and acceptance evidence. Legacy role-only panels are not the production acceptance target.

An event is strict UTF-8 JSON of at most 8192 bytes, with exactly these envelope keys:

`v=1`, `recording_id`, `sequence` (1..9007199254740991), `domain` (`kind=device|setup`, `id`), `connection_id`, `epoch` (0..9007199254740991), `driver_kind` (`osa|voltage|gain|pm400|mdt|fiber`), `source_kind=real`, `kind` (`sample|operation|authority|fault`), `recorded_utc` (UTC ISO-8601 with Z and millisecond precision), `elapsed_ms` (finite nonnegative JS-safe number relative to this recording), `read_interval`, `values`, `details`.

`read_interval` is null when no actual read interval is known; otherwise its exact keys are `started_utc`, `ended_utc`, `elapsed_ms`, `time_quality` (`normal|discontinuous`). The monotonic elapsed interval is nonnegative; a backward wall-clock interval requires discontinuous, not fabricated monotonic UTC. Read intervals describe host queries, not instrument acquisition timestamps.

`values` is a list of at most 32 unique fields. Each has exactly `field` (1..64 lower-case ASCII letters/digits/dot/underscore; begins with a letter), `value` (finite number, Boolean or null), `unit` (nonempty printable text <=16 characters or null), `semantics` (`measured|readback|requested|estimated|state`), `quality` (`fresh|stale|unknown|error`), `source` (nonempty printable text <=64 characters or null), `age_ms` (finite nonnegative JS-safe number or null). Boolean values require state semantics and a null unit; other semantics require numbers or null. State values are Boolean or null and unitless. A fresh/stale value requires a non-null value/source/age; null is never a fresh zero. String-valued state/context belongs in details, not in numeric values. No absolute monotonic clock crosses processes.

`details` is bounded JSON metadata: object root, <=256 visited values, <=6 nested containers, <=32 entries per object/list, strings <=1024 printable characters. No lease token, ownership nonce, live session ID, password or secret is persisted. Unknown envelope/value/interval keys, duplicate JSON keys, invalid UTF-8/surrogates, non-finite values, unsafe integers, cycles, oversized or deep input fail before admission. The codec validates data, not authenticity: only the owning driver/Host pipeline may supply production events.

Worker outbox: at most 8 active recordings, 64 unacknowledged events/512 KiB each, plus one reserved fault terminal. The stream is bound to recording/domain/config revision/current connection/epoch. One drain returns at most 4 events (<=32768 event bytes) and a terminal marker; each reply must fit the existing 65536-byte envelope. Drain is non-destructive and stable until exact sequence/hash acknowledgement; it never calls hardware. Duplicate ack is idempotent; future/mismatched ack fails. A full queue freezes the stream with `OutboxFull` and exact accepted/dropped-next sequence evidence, not silent overwrite. An unsuccessful encoding similarly freezes only recording and reports the original reason.

Durable layout is Result/<name>/<recording_id>: immutable manifest.json; append-only events.jsonl; immutable final.json only after flush/close, containing manifest/event hashes, exact byte length/count, last sequence and terminal reason. A clean stop seals status complete; an outbox gap/write/close failure seals status partial when possible. A missing final marker after restart is interrupted; recover only an independently validated prefix and never truncate/rewrite original evidence. Active historic reads return committed-prefix length/hash, not an unbounded changing stream. Different archive types have distinct namespace markers and cannot claim each other's directories.

### Task 1: Strict, portable telemetry event codec

**Files:** App/worker/telemetry_schema.py; App/tests/test_telemetry_schema.py.

**Interfaces:**
- Consumes literal event dictionaries/UTF-8 bytes with the contract above; no clock/driver/storage dependency.
- Produces `encode_event(event: dict) -> bytes`, `decode_event(payload: bytes) -> dict`, and `TelemetryError(ValueError)`. Encode returns a defensive, validated JSON byte representation; decode accepts arbitrary property order but rejects duplicate keys. Both enforce the same semantic/size bounds.

- [x] Step 1: Write literal round-trip and negative tests. Catch removal of real-only gating, duplicate-key rejection, field semantics/freshness rules, byte rather than character size accounting, privacy-key rejection, timestamp validation, nested/cyclic/nonfinite/unsafe-integer checks and alias isolation. Include true numeric zero, unknown null and negative current without coercion. A pure codec test must not count as driver/hardware acceptance.
- [x] Step 2: Run VISA -B -m unittest App.tests.test_telemetry_schema -v. Expected: RED because the production codec does not exist; use a missing-module assertion so test collection itself stays valid.
- [x] Step 3: Implement the named codec with bounded prevalidation, strict duplicate-aware decoding and explicit schema whitelists. Do not import a driver or introduce a selectable execution backend.
- [x] Step 4: Run the codec test and complete App/Code/Node/Cargo regression suites. Expected: exit 0; existing intentional fault-injection logs and compiler warnings remain disclosed, not suppressed.
- [x] Step 5: Scoped commit and ledger with source range/commands/counts; Task 1 is only the contract, not a usable recorder.

### Task 2: Driver evidence projection and bounded worker outbox

**Interfaces:**
- Consumes Task 1 encode_event; admitted Scheduler observation context/status and terminal public-driver outcomes. Capture hooks execute only after the existing scheduler context/revision acceptance fence.
- Produces `TelemetryOutbox.open(recording_id, binding, interval_ms) -> dict`, `publish(recording_id, event) -> None`, `drain(recording_id, after_sequence, limit=4) -> dict`, `acknowledge(recording_id, sequence, sha256) -> None`, `close(recording_id, reason) -> dict`; projection `project_values(driver_kind, status, *, sample_age_ms) -> list[dict]` and private worker methods `open_telemetry`, `drain_telemetry`, `ack_telemetry`, `close_telemetry`, authenticated with current ownership nonce/global context and explicit nested binding.
- Binding includes domain, config_rev, connection_id and epoch. Result IDs and recording config never grant a lease. Host-only methods are excluded from client ExecuteParams.

- [ ] Step 1: Add RED tests for finite exact VoltageStatus (eight voltage/current channels with requested values separate), Gain partial/stale fields and current readback semantics, MDT cached actual voltage age, Fiber mapped voltage/estimated position/baseline invalidation, and PM400 unit/configuration/sensor changes. Test accepted observation vs stale revision, default1000 ms but actual2500 ms source cadence, command/authority events without decimation, 64-entry overflow, idempotent drain/ack, two streams and old-context stop; expect no serial/VISA writes from outbox or projection.
- [ ] Step 2: Run the new tests plus scheduler/v3/domain/controller tests. Expected: RED on absent outbox/projection/typed fetch integration, never a permissive instrument mock assertion.
- [ ] Step 3: Implement projection/outbox and scheduler accepted-event hook with bounded in-memory work only; catch recorder errors without changing hardware lifecycle results. Keep the lock order Condition -> registry -> outbox, never await file/IPC under them. Retain per-field source age/revision and null for unknown; derive UTC read boundaries only from actual measured interval. Do not use viewing time as a new sample time. Private drain runs outside instrument lanes and cannot displace safety slots.
- [ ] Step 4: Add typed PM400 `fetch_value` using only public `measurement.get_configuration()/fetch()` and capability-gated read-only context queries. An explicitly recording, connected PM400 may fetch no faster than its selected interval; never call measure/read/configure/initiate/abort. Its cadence is a distinct admitted read-only observation responsibility, bounded with the existing per-domain scheduler and connection fences. Record FETCh read time, not invented independent instrument acquisition time. OSA only attaches already completed archive refs to operation events; no recording loop issues a sweep/read_trace implicitly.
- [ ] Step 5: Run all four suites. Expected: exit0 with finite transport transcripts, no hardware. Commit and ledger.

### Task 3: Durable append-only recording actor

**Interfaces:**
- Consumes Task1 event JSON and Task2 exact consecutive sequences/hashes; trusted provenance from Host registration, not frontend claims.
- Produces `RecordingStore::open(root: &Path, host_id: &str) -> Result<Self, HostError>`, `begin(origin: RecordingOrigin, options: RecordingOptions) -> Result<RecordingRef, HostError>`, `append(id: &str, events: &[Value]) -> Result<CommittedPrefix, HostError>`, `finish(id: &str, reason: RecordingEnd) -> Result<RecordingEntry, HostError>`, `list(domain: &DomainRef, offset: usize, limit: usize)`, `read(id, offset, length)` and `manifest(id)`. Actor has 8 queued jobs and bounded nonblocking admission; safety stop uses reserved metadata admission, not the ordinary queue.
- RecordingOrigin preserves host/domain/device identity/config revision/original start operation ID/source real. RecordingOptions contains validated name, interval_ms and revoke_on_failure. References contain no session/token/command authority. At most8 active recordings; persisted history is paginated with limit1..16 and chunks <=16384 bytes. A recording is limited to256 MiB; reaching the ceiling ends visibly with partial capacity evidence, never rollover or silent loss.

- [ ] Step 1: Cargo RED tests literal append/reopen/hash/exact CSV provenance; duplicate same sequence is idempotent, conflict/gap fails; fsync/append/final-marker failure preserves partial evidence and original bytes. Test blocked writer/8 ordinary slots/reserved stop, foreign namespaces, traversal/reparse/hardlink replacement, active committed prefix and interrupted trailing JSON without destructive repair. Inject faults only at finite owned file boundaries.
- [ ] Step 2: Implement separate recording_files/recording store modules with strong native directory pins; exact-handle exclusive creation and immutable metadata commit. Append a newline-delimited event only after full validation, and sync before returning CommittedPrefix; a partial write is terminal, not retried as a new event. Keep content hashing streaming and bounded. Prefix recovery validates sequence/shape/bytes; final marker mismatch is corruption, not a successful interrupted recovery.
- [ ] Step 3: Run Cargo fmt/check/full tests and all existing suites. Expected: exit0, existing OSA archive integrity tests retained. Commit and ledger.

### Task 4: Host ownership/lifecycle and typed recording routes

**Interfaces:**
- Consumes Task2 private methods/outbox and Task3 store/actor. Existing LeaseBook validates current session/token/epoch/config and cleanup tickets.
- Produces authenticated Host routes `start_recording`, `stop_recording`, `list_recordings`, `recording_manifest`, `read_recording`; start/stop return a ledgered request operation/reference. Native/frontend client names are `startRecording`, `stopRecording`, `listRecordings`, `recordingManifest`, `readRecording`. Start params are request_id, domain, lease_token, control_epoch, config_rev, name, interval_ms, revoke_on_failure; stop requires request_id plus the same authority fields and recording_id. Historic routes need a current authenticated observing session/domain but no old lease. Stop is not disconnect; safety/close can retire a stream internally regardless of UI authority.

- [ ] Step 1: Native pipe RED tests start once/two observers, duplicate request mismatch, shared reference/status/hash and page-independent append; observer forbidden start/stop; close observer leaves recorder/owner running. Owner release/expiry/revoke/Host stop fences ingress immediately and drains admitted prefix once. Fault policy false stops only recorder; true requests existing domain cleanup without changing piezo hold policy. Worker death/disrupted ack never replays driver calls; restarted Host exposes interrupted history and no live recorder.
- [ ] Step 2: Implement recording_service.rs manager with max8 streams and one bounded drain in flight per stream. A250 ms metadata drain tick does not create extra hardware polling. Native disk jobs are not awaited while holding lease/admission/worker query locks. Stop publishes a fence before file flush; hardware safety dispatch never waits on disk flush. Store reservation/manifest precedes worker outbox open, and failed open records a start failure rather than active success.
- [ ] Step 3: Route drain -> validate binding/order/hash -> durable append -> exact acknowledgement. Publish immutable Recording state/reference/end reason to EventHub. A missed ack resends only the stable data batch, never the hardware action. Recording cannot survive change of configuration/connection/epoch/owner; pre-fence accepted rows may flush after the fence but no new-owner sample can enter. Data-root changes apply only after normal Host restart, matching current archive semantics.
- [ ] Step 4: Full suites and owned native Host process checks. Expected: exit0; a live external Host remains untouched if its guard is held. Commit and ledger.

### Task 5: Simple per-device Record/Stop/history/export and delivery verification

**Interfaces:**
- Consumes Task4 client routes/Record references/events and Task3 verified prefix/chunk/history metadata; future remote clients retain host_id/domain keyed routing.
- Produces one Record/Stop control plus concise destination/history/Save export on Voltage/Gain/PM400/MDT/Fiber instance pages. OSA preserves Read trace/native capture export and never becomes periodic sweep automation. Advanced recording interval/failure policy is collapsed, not extra toolbar clutter. Export includes events CSV/JSONL, original manifest, final or committed-prefix metadata and hashes.

- [ ] Step 1: Node RED tests same owner recording survives page change; observer can view/save but not Record/Stop; old event/reference and duplicate labels cannot cross Host/domain; incomplete/stale/estimated/requested values remain labeled, no synthetic zero on errors. Test incremental pagination/chunk/hash limits, disabled actions after lease loss, native chooser cancellation and exact sample exports, not decimated GUI plots.
- [ ] Step 2: Implement recording.js/view integration and native authorized save/export path. Only one drain/history request per view; close/cancellation drops view responsibility without stopping owning Host recording. Historical archive selection grants no connection or control; failure is prominently partial with original error, not a success toast.
- [ ] Step 3: Full Code/App/Node/Cargo regression, debug/release build and exact NSIS compiler-input/resource/hash verification. Expected: exit0, no claim of installed/native-window/hardware acceptance from build-only evidence.
- [ ] Step 4: Only after offline gates and separately disclosed scope, validate currently identified safe real telemetry/OSA data without nonzero outputs or motion. Unresolved Gain temperature/probe and absent PM400/MDT remain pending, not fabricated acceptance. Native UI and actual two-computer remote proof remain separate; do not install over the normal App automatically.
- [ ] Step 5: One independent whole-plan review, one TDD fix pass for Critical/Important, disclose deferred minors/rulings, commit acceptance evidence and preserve recoverable review material before exact plan-workspace cleanup. Do not merge/push until repository is identified and the public-bottom-layer release is separately verified.

## Self-review and next increment

The five tasks form one recording pipeline: codec -> bounded worker evidence -> durable actor -> owning Host lifecycle -> simple views/export. Every Review Focus input has a named owner/test step. Event byte/count/sequence limits agree across tasks; Host acknowledgement requires durable prefix evidence, not receipt or GUI success. Sample projection retains driver semantics while typed fetch avoids PM400 measurement-setting writes. Safety and recording stop are separate responsibilities.

Public-bottom-layer release must still include the separately approved TLS/multi-Host transport, truthful physical-device acceptance and the subsequently selected private-runtime/clean Windows deployment gate; this plan does not narrow the overall goal or claim those complete. Origin is now the operator's ZQ200630/Yanglab_Instrument_Manager repository (currently public); publication awaits privacy/history review and full common-framework acceptance. The desired main -> codex/pic-desktop and codex/laser-1060-desktop workflow is retained. No publication is attempted by this telemetry increment.
