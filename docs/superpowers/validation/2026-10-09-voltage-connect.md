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
