# Native 8-channel voltage connection investigation

The operator reports COM3 / CH340 Connect & verify failing with
`Responsibility("voltage lifecycle canceled/faulted")`. Original authenticated
Host metadata is preserved in Result/voltage-connect/pre-repair-host.json, with
the original terminal operation and current retained domain saved separately.
The CH340 description identifies the USB adapter, not the instrument protocol.

## Causal findings and bounded corrections

The native voltage decoder needs 102 bytes before choosing initial alignment;
each reader poll requests at most 68. A healthy first poll can therefore lack a
complete decoded observation. The no-first-observation watchdog currently uses
monotonic zero as its last-receive time. Because the Worker clock starts before
device connection, opening after the default 3.5-second recovery window can
immediately fault. Anchor initial silence to the current connection/reader start;
retain last-valid-frame timing and all existing startup/recovery bounds.

NativeBackend and Scheduler production assembly currently creates separate
SystemClock origins. A missing native observation timestamp falls back to the
earlier backend clock, potentially putting it ahead of the Scheduler and hiding
the original failure behind EvidenceError. Share one clock through production
assembly; retain rejection of genuinely future observations.

Lifecycle failure must retain its actual transport or timeout error rather than
replace it with a generic cancellation string. Output zero attempts and immutable
cleanup receipts remain separate evidence. No limit, ramp pacing, zero-on-fault,
or retained-resource rule may be weakened to make Connect appear successful.

Voltage poll currently starts a 10 ms read even when no bytes are buffered.
Native serial cancellation may legally remain pending, in which case the strict
serial responsibility fence prevents the next operation. Polling should first
use the existing read-only receive-count API: zero bytes means no read; available
bytes permit at most min(68, count), with the original bounded deadline. Receive
count errors remain faults; there is no speculative-read fallback or relaxation
of pending-operation ownership. This avoids turning a normal stream gap into an
unnecessary asynchronous cancellation, without claiming every cancellation fails.

## Verification and delivery plan

Use finite injected serial transports to reproduce delayed-open, fragmented
initial frames and real failure bounds before changing the driver. Cover a fresh
baseline on reconnect and read-only probe, and preservation of original failures.
Use a bounded Worker observation regression to reproduce mismatched clock origins
and verify shared-clock delivery without relaxing evidence validation.

After source review, run native offline and frontend/package regressions, then
build and qualify one successor portable package containing both this correction
and the completed Gain 5 Hz update. Replace the old Host only after normal GUI
exit and current-context resource-release evidence. No force termination or
automatic instrument reconnect is part of delivery. Actual hardware diagnostics
remain subject to the project's separately authorized stages.

## Focused verification

Six startup/fault regressions first reproduce the former lifecycle string;
three polling regressions reproduce reads on an empty buffer, over-requested
byte counts and missing receive-count failure propagation. After the correction,
all 32 Voltage tests pass, including aged first connection, read-only probe with
zero output writes, a new baseline after reconnect, initial fragments, fixed
startup deadlines under endless partial traffic, the unchanged established-frame
recovery boundary, close preemption and separate initial/cleanup errors.

The shared-clock regression uses the actual injected native Voltage session and
a finite failed zero write. The original clock assembly rejects its fallback
observation as a future timestamp; the corrected shared-clock assembly preserves
the fault snapshot and terminal request/context. Genuine future evidence remains
rejected. All 138 Worker tests pass; final telemetry after the buffered-read
driver correction passes 29/29. Independent frozen-source review reports no
P1/P2 and diff checking is clean.

Evidence: Result/gain-5hz/voltage-startup-{red,green}.log,
voltage-worker-clock-{red,green}.log and voltage-worker-telemetry-final.log;
Result/voltage-connect/native-poll-red.log and native-green.log.
The historical Host terminal record had already lost its initiating transport
error, so these reproduced software defects cannot retrospectively prove the
exact COM3 transport event or physical output state. No real port was opened by
this investigation; hardware acceptance remains separate.

Root frozen-source full App/scripts/test-native.ps1 -Offline completes with
exception-aware wrapper exit 0: 717 Rust tests, 491 frontend tests and all 13
packaging assertions passed. Evidence: Result/voltage-connect/full-native.log.
The 14 mounted Chromium scenarios from the Gain 5 Hz candidate remain applicable:
this voltage correction changes no web production files. All output diagnostics
remain unrun; the old App/Host still own the failed draft until normal release.

## Qualified successor and release wait

Candidate 05 contains both the Gain 5 Hz change and the Voltage/Worker correction:
Result/native-package/gain-console-20261009-05/portable, package revision
0.1.0-2da2e7a6255b and source
tree-2da2e7a6255b345122bab10489ee409a2e6e5d832c85045c7ae7787197bf2d38.
The 29-file / 3-driver / 18-pin package contract, AMD64/static CRT checks,
disarmed exact packaged Worker startup/EOF exit and all pristine ZIP hashes pass.
GUI SHA256: d53f3aeb6fab8dd5a47bb70342399854aa73c2a825cc578f401123dc821f56e2.
Host SHA256: 0b02bb016073c95b01680043f3943e831f03ebcc9b99366de983f33c711852e2.
Worker SHA256: 9f2f6dff4bf325ea72c8bd9b59e2cb013ad55caa658a3ba7704554e60674e254.
ZIP SHA256: 9ff81d9db0d0775b010e3810ec7aa0528ad1163a7364307ae4b1db19a19bcdcc.
Evidence: Result/voltage-connect/{build.log,package-check.log,pe,portable-worker,
zip-qualification.json}.

Current release check still finds candidate-03 GUI 21744, Host 80984 and Worker
54620. Gain remains READY with a connection; the voltage draft remains FAULT
with responsibility and a connection. Neither a completed new release nor App
exit is confirmed. Candidate 05 is ready but has not been launched; no forced
termination, old-owner replacement or instrument reconnect occurred. An async
operator request asks for normal Gain disconnect, voltage draft cancellation
(the existing zero-on-close lifecycle), then normal App exit. This is required
by AGENTS.md's pending-owner retention rule. Evidence:
before-switch-host.json, pre-switch-processes.json, current-release-check.json.

## Old Host shutdown aggregation failure

After the operator exited the GUI, authenticated current-context checks confirmed
all three current domains disconnected, without connection IDs, pending work or
control owners. Gain and Voltage each retained a completed disconnect receipt,
empty unreleased lists and successful ordered cleanup steps. The formal Host
shutdown then failed while normalizing two independently scoped Voltage cleanup
receipts. The previous normalizer rejected the second nonnull voltage_zero
record before storing or delivering the global terminal. The Host-owned Worker
handle subsequently confirmed exit code 0, but the old Host could no longer
recover that missing global cleanup terminal or finalize its startup record.

The successor keeps every original receipt in native_cleanup. Its legacy singular
voltage_zero projection is populated only when exactly one receipt supplies it;
multiple receipts produce null rather than selecting one or claiming aggregate
zero. All report bounds, field validation, retained-resource conflict checks and
physical_zero_verified=false remain intact. A finite actual WorkerRuntime pipe
regression reproduces the old rejection with two valid receipts and successful
peer exit, independently of hardware or GUI readings.

Evidence: Result/voltage-connect/operator-exit-host.json,
operator-exit-processes.json, stop-retained-host.json, stop-retry-retained.json,
retained-startup-record-summary.json and multi-voltage-runtime-red.log. Original
unknown startup metadata is preserved separately; it must not be relabeled as a
successful global release. No successor Host has been activated at this stage.

Frozen-source verification passes: focused Host library 267/267 and bounded
transcript fixture 1/1, followed by root App/scripts/test-native.ps1 -Offline
with wrapper exit 0, 722 Rust tests, 491 frontend tests and 13 packaging
assertions. The earlier multi-voltage-host-green.log contains three stale debug
Worker identity prerequisite failures and is not qualification evidence;
multi-voltage-host-final.log is the completed focused run after rebuilding the
test prerequisites. Final full evidence is final-full-native.log and
final-offline-summary.json. Independent review finds no P1/P2 in the minimal
normalization correction or finite test fixture. No hardware diagnostics ran.

## Final successor qualification

Candidate 06 supersedes unactivated candidate 05 and includes the shutdown
aggregation correction alongside Gain 5 Hz and the Voltage connection fixes:
Result/native-package/gain-console-20261009-06/portable, package revision
0.1.0-b5621ac72000, source
tree-b5621ac7200066af2c4a10f759b19dba36fe7fecb03ab040554413fb3cfa2f0a.
Its 29 files, three driver packages and 18 pins pass the source/package contract;
all executables are AMD64 with static CRT and no Python payload or imports.
The exact packaged Worker passes disarmed ping and clean EOF exit 0 with only
Windows/System32 on PATH, zero connected domains and no Host/GUI started.
All 29 pristine ZIP entries match their corresponding qualified files.

GUI SHA256: 4c24f35b8e8235c600824edd4b61a3810027480648d650b1f4bc4152da802588.
Host SHA256: 2147c4f3177f03ecb1d630b55172b225997a4b2cf5a9b5e262c006d9bb24db69.
Worker SHA256: 95d00d1732f5673ae7ae57bf48ddebe8712b0ae039d7ec7f6f47121d88c2df4a.
ZIP SHA256: da531f0c3736508767729a9230b74f59b38a714791044531fdf33038cf14a019.
Evidence: final-build.log, final-package-check.log, final-pe,
final-portable-worker and final-zip-qualification.json under Result/voltage-connect.
The final candidate is qualified but remains unactivated pending explicit
administrative recovery of the metadata-only old Host and preservation of its
unresolved original startup records. Instrument reconnect is not part of that
recovery. Physical acceptance remains separate from these software checks.

## Authorized recovery and activation

The operator approved the reviewed administrative recovery by requesting
"启动新版". The frozen one-off administrative-recovery.ps1 (SHA256
2e4ee66186996c948089d25f2f33041140e243b9eb38c9f0aa27abae834ceb22)
rechecked fresh authenticated current-context release receipts and the exact
Host-owned Worker exit code 0, terminated only the pinned metadata-only old Host,
then held a newly created machine-wide Host guard during no-overwrite archival
of the two original unresolved records. Their original bytes and hashes match;
the old Worker record remains identified, with no reconstructed global terminal
or physical-zero claim. Archive: AppData/Roaming/edu.wustl.yanglab.silconsole/
recovery/host80984-1541988093ff44b9add352aabeffa36f.

After another source/package contract check, candidate 06 launched with GUI
PID 41376, Host PID 37840 and Worker PID 17688. All three running image hashes
and paths match the qualified package. The GUI has a visible responsive window;
authenticated snapshot and worker_status both report ONLINE with new boot
563e8767b32198a699ca6eb4c505b238 and no startup error. Both initially configured
domains (Laser and Gain) are DISCONNECTED, with no connection, responsibility,
pending work or request IDs. No instrument reconnect or output command was
issued during recovery or activation. The new startup record identifies the
qualified candidate-06 Worker normally; it does not relabel the archived owner.

Evidence: administrative-recovery-execution.json, its completed.json and copied
original records, final-pre-launch-package-check.log, final-launch.json,
final-running-host.json, final-running-processes.json and
final-delivery-ready.json under Result/voltage-connect. Independent read-only
review confirmed recovery bytes, receipts, running identities and initial Host
state. Candidate 06 is now activated; physical device acceptance remains unrun.
