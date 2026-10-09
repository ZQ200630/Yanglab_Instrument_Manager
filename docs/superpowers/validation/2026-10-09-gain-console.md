# Gain console dashboard and native controls

Date: 2026-10-09. Baseline: 25dce850, codex/merge-desktop.

## Delivered behavior

The Gain console uses a rolling temperature plot and a separate overall-status
card above equal Temperature Control and Current Control cards. Routine per-field
Fresh/age labels are removed. Disconnected, unknown, stale or faulted evidence
still has a truthful shared notice; cached numeric readings are not relabeled as
fresh or physical output measurements. Controller current setting is named
explicitly rather than claimed as independently measured injection current.

The plot offers 1/5/15-minute windows, uses real observation timestamps and
keeps at most 3600 samples for a 250 ms publication cadence. Invalid evidence,
lost communication and connection changes break the line. Timer repaint does
not manufacture samples; collection continues across routes. Y limits adapt to
observed temperature and target without hiding a literal zero reading.

Temperature/current/PID edits remain local until Apply or Enter. PID values come
from one automatic native getter per connection, have their own timestamp and
are never filled with invented defaults. PID and ramp options are expandable.
Dirty drafts, details, selection, focus and scroll survive telemetry and route
changes. Responsive layout was inspected at 1440, 960 and 800 px, including no
horizontal overflow and visible controls.

Soft start and smooth changes default on, with 1 mA steps and 100 ms intervals.
Starting current automatically verifies TEC on and waits for five continuous
seconds within target +/-0.2 degC. It does not enable TEC automatically.
The native controller's Q=1 activation resets its current setting to 3 mA;
the driver obtains the real readback before ramping to the requested target.
The default 30 s start timeout covers the entire compound operation, including
stability, enable, ramp and final verification. Impossible plans fail before
activation; insufficient remaining time after activation retains shutdown
liability. Current remains limited to 0-200 mA and target to 15-40 degC.

One original driver generation spans preflight and every action adapter and
packet. Off cancels waiting/ramping before any delayed normal command can resume.
Current is disabled before TEC. A stronger TEC Off requested while Current Off
is pending queues once behind its known terminal receipt and fresh epoch.
Uncertain outcomes retain their original IDs and never replay output changes.
After a verified same-connection STOP_HELD Off, the next explicit normal operator
command resumes metadata, validates boot/context/lease again, and sends its
captured action once; there is no separate normal Resume step or automatic
restoration of outputs.

Live observations use the existing native Gain cache without instrument I/O or
blocking on a compound action. Cached publication is 250 ms during idle and
active Gain operation, with field/PID ages rebased for cache residence. Observed
progress counts only acknowledged ramp steps and cannot authorize completion.

## Verification

Behavioral RED/GREEN covered native PID/current limits and budgets, original
generation through Lazy/Typed adapters, independent cache age, quiescent epoch
rebinding, Off cancellation and compound-operation fault liability. UI RED/GREEN
covered drafts, actual history, gaps, exact current receipt correlation and
canonical Canceled handling. Independent frozen-source review found no remaining
Critical/Important issue in Native/Worker and frontend; root separately reviewed
Host/catalog and presentation.

Root final App/scripts/test-native.ps1 -Offline exited 0: **689 Rust tests,
480 frontend tests, 13 packaging checks**. Evidence:
Result/gain-controls/offline-final.log and offline-summary.json.

Root mounted production Chromium regression exited 0 with nine groups: initial
PID/read/edit/Enter/focus, background history and outage gaps, interrupted start
and subsequent On, smooth ACTIVE changes, uncertain Off without replay, uncertain
metadata resume, typed OutcomeUnknown start, queued TEC Off, and Off interrupting
delayed metadata resume. The Host is a bounded injected protocol double with no
native/hardware reachability. Evidence: Result/gain-controls/browser-final.log;
test source App/tests/browser/gain-control.mjs.

Layout evidence: Result/gain-controls/dashboard-1440.png, dashboard-960.png,
dashboard-800.png and visual-layout.json. Plot data in these screenshots is a
finite illustrative UI fixture, not a hardware acquisition.

## Delivery and physical boundary

The exact portable candidate is
Result/native-package/gain-console-20261009-01/portable, revision
0.1.0-b7e9c6dde347, source
tree-b7e9c6dde347f64959fdde80c2f13c1672c647ac2c6a1f2e6e42ff76e8d904ec.

- GUI SHA256: 92bea81ffb74f661b64fbd99335e0b335aa0d59e3203388611ed1fac63a80569.
- Host SHA256: 3e6d6f308e3a531bc5a285704945d6d6afc737e5f876c5c030440698b1bf6e96.
- Worker SHA256: ff9d4557395362cce10136d343f139bb29aa99c66f8425add6a4a2cedb2451b5.
- Pristine ZIP SHA256: 507a092430e2f55f3ce2474fb5a19e1851a5201542e0afdbd1d2908ef9cfaf94.

The package passed its closed 29-file contract, all three driver packages and
18 pinned payload hashes. Each ZIP entry matches its pristine portable file.
All executables are AMD64 with no Python or dynamic MSVC CRT import. With only
System32 on PATH, the exact packaged Worker returned matching real/protocol 3
and disarmed empty startup identity and exited cleanly on EOF. This check did
not start a Host, GUI or instrument. No installer was built or installed.
Evidence under Result/gain-controls: build-final.log, package-check.log,
pe/pe-qualification.json,
portable-worker/91231f4cc63049a98f2e642cbe9c0068/qualification.json and
zip-qualification.json.

Fresh authenticated cached metadata in owner-before-switch.json confirms the
older gain-connect-20261009-02 App still retains a READY Gain connection; Laser
is DISCONNECTED. App replacement waits for normal operator disconnect/exit and
authenticated release evidence. No agent-issued
instrument enumeration, serial/SDK connection or output diagnostic ran for this
task. Software tests do not establish physical sensor accuracy, current behavior
or independently measured output zero. No clean-Windows qualification is claimed.

The prior Laser single-scan reviewed production record set remains empty and is
outside this Gain task.

## First switch and wire-format regression

After the operator requested the switch, the older GUI still existed and retained
Gain. A normal CloseMainWindow request to its verified PID 69952 invoked the
program's existing original-client close guard. GUI exit was confirmed. The first
post-close cached snapshot was context-changing UNKNOWN; it was not accepted as
release evidence. A later authenticated snapshot confirmed DISCONNECTED, no
connection or pending work, responsibility false and AVAILABLE control. The
matching ClientClosed cleanup recorded current_off, tec_off, transport_close,
no errors and no unreleased resource. Guarded stop-if-disarmed then confirmed
Worker resource release and successful process exit before the old Host exited.
No forced termination occurred and no independent physical zero was inferred.

Package 01 launched as GUI 74296 / Host 17160 / Worker 54232, with matching
qualified executable hashes, a new boot ID and ONLINE Rust Host. Later metadata
already contained a new Gain connection; the first post-launch no-restoration
assertion was therefore not satisfied. No agent-issued instrument Connect was
sent. Evidence: normal-close-request.json, owner-after-normal-close.json,
owner-released-current.json, safe-stop-old-host.json, new-processes.json and
switch-confirmed-01.json under Result/gain-controls.

The operator then reported an uncertain-operation banner. The same authenticated
metadata contains a successfully completed Gain Connect, request
a0dddd5681ce494588ff67e1dc7a05f3, correct current connection, no pending command,
READY driver, fresh field observations, no driver fault and PID null.
The UI's new identity check compared JSON.stringify(record.domain) against the
route domain. Rust serde_json::Value returns {id,kind}; parseRoute constructs
{kind,id}. This deterministically rejected the successful Connect and blocked
the automatic PID getter. Query-original and several safety/release identity
checks shared the same order-sensitive comparison. The browser fixture reused
JavaScript insertion order and had missed the real wire boundary.

A canonical Rust-order mounted transport fixture now reproduces this specific
failure before correction. The corrective comparator must ignore only object
key order while retaining every key, type, value and array position; boot,
session, connection, epoch, lease and request fences remain exact. Gain uncertain
presentation has separate RED/GREEN coverage: one actionable shared notice,
warning status preserved, and normal operation uncertainty is not labeled as an
unconfirmed release. Actual retained/failed disconnect still is.

## Corrected package

The shared sameJsonValue comparator now checks all own keys recursively, exact
primitive types, object/array distinction and array length/order. Gain normal,
safety, cancellation and resume checks, original-operation recovery, current
result display, connection release and setup release tickets use it. Intentional
PID connection binding and native cached observer epoch rules are unchanged.
Different IDs, connections, sessions, epochs, missing/extra fields and changed
array positions still fail the regression tests.

Root frozen-source verification: **490 frontend tests**, **10 canonical-wire
Chromium groups**, and **13 packaging checks**, all exit 0. The corrective change
does not modify Rust; its preceding complete native regression remains 689 tests.
An independent reviewer ran 110 focused tests and replayed the exact saved
completed Connect as an offline status response: resolved=true, one resync and
no instrument command. Evidence: canonical-frontend-final.log,
canonical-browser-final.log, canonical-packaging-final.log under
Result/gain-controls; original canonical RED and unit RED logs under
Result/gain-failed/ui-canonical-*.log.

Corrected candidate: Result/native-package/gain-console-20261009-02/portable.
Revision 0.1.0-bdee06b28b8e, source
tree-bdee06b28b8e01f3bb67487d45e56ac8b0accdb90146a246ccc0fdf77a3d7a84.

- GUI SHA256: 52dadf59be50a1466e2e41d63a37921044a8f4a9ba6fed2c0d0c22cb0c22ad74.
- Host SHA256: 3e6d6f308e3a531bc5a285704945d6d6afc737e5f876c5c030440698b1bf6e96.
- Worker SHA256: 25acb1804f689b11dca425879f0a9908ef94b21608f7fa69ddf1c8b699a2c8c3.
- ZIP SHA256: 467cb015b52330f2285fd2e6af1d2a256efdf8b7c94d9b880ca8f0628daeae75.

The closed 29-file/3-driver/18-pin contract, AMD64/no Python or dynamic MSVC CRT,
disarmed exact packaged Worker System32-only startup/EOF exit and every pristine
ZIP entry hash pass again. Evidence: canonical-build-final.log,
canonical-package-check.log, canonical-pe/pe-qualification.json,
canonical-portable-worker/b20f1b2841dc495d8f280c5343568ead/qualification.json,
canonical-zip-qualification.json under Result/gain-controls.

The corrected switch is confirmed: GUI 55048 / Host 17320 / Worker 20144 all run
from candidate 02, with a responding visible GUI and new authenticated Rust Host
boot 7a078cf70904b6b0e3c5bcfa3c2975ad ONLINE. Both instrument domains are
DISCONNECTED with no connection; no instrument command was restored. The prior
GUI's normal close guard and matching Gain cleanup receipt preceded the guarded
Host resource-release/successful-exit receipt and new launch. The transient
context-changing UNKNOWN snapshot was again not accepted as release evidence.
Evidence: canonical-after-normal-close.json, canonical-owner-released.json,
canonical-safe-stop.json, canonical-launch.json, canonical-new-app-online.json,
canonical-new-processes.json and canonical-switch-confirmed.json under
Result/gain-controls. Actual operator Connect/PID/current/TEC acceptance remains
separate from the verified software-boundary correction.

## Gain keyboard interaction

The operator requested Enter submission for the target temperature, target
current and PID inputs, plus direct Tab navigation between the two targets.
The mounted-browser RED showed Tab selecting Apply temperature instead of
Target current (Result/gain-failed/ui-keyboard-tab-red.log).

Plain Tab and Shift+Tab now alternate between the target inputs without applying
either draft. Enter delegates to the same corresponding Apply button and keeps
its validation, pending, authority and uncertain-outcome gates. Target inputs
and PID fields ignore held/repeated Enter, modifier chords and IME composition.
Typing, arrow adjustments and focus changes remain local edits. Applying a
current target does not turn on either output; an already enabled current keeps
the existing native Smooth changes ramp. Inline hints describe Enter and Tab.

Frozen production source passed all 490 frontend tests and 13 packaging checks.
Independent keyboard/Gain/focus source review found no P1/P2 issue and passed
15 focused tests. Rust driver and Host lifecycle logic are unchanged by this
keyboard update; no hardware output diagnostic was performed.

Candidate Result/native-package/gain-console-20261009-03/portable has revision
0.1.0-cbf2fd844274 and source
tree-cbf2fd84427404db643c04e05ad4e4007616c3549cce39191d0a7b9f075278ce.
The 29-file/3-driver/18-pin contract, AMD64/no Python or dynamic MSVC CRT,
disarmed packaged Worker startup/EOF exit on System32-only PATH and all 29
pristine ZIP entry hashes pass. ZIP SHA256 is
e112939b0dca278d84732ed6e1624e77700abc731519217bee6104fed79bcf70.
Evidence is in Result/gain-controls/keyboard-frontend-final.log,
keyboard-packaging-final.log, keyboard-build-final.log, keyboard-package-check.log,
keyboard-pe, keyboard-portable-worker and keyboard-zip-qualification.json.

Final mounted Chromium verification passed 13 scenario groups (exit 0), including
both Tab directions, all three PID Enter targets, current Off/set and On/ramp,
no edit/arrow/focus writes, modifier/composition/prevented/repeated keys and
duplicate/pending/unknown blocking. Enter submission itself already worked in
the preceding build; this change confirms it at the mounted wire boundary and
adds composition/modifier guards and visible instructions. Evidence:
keyboard-browser-final.log; visual check:
Result/gain-redesign/browser/control-panels-keyboard.png.

Before switching, authenticated metadata confirmed Gain READY, current Off,
TEC Off, no pending work and Laser DISCONNECTED. The exact candidate-02 GUI
received a normal CloseMainWindow request but remained responding after the
15-second exit wait. Later metadata still showed its original Gain controller
and no cleanup attempt; this is not resource-release evidence. No forced
termination or replacement was attempted. Evidence:
keyboard-before-switch.json, keyboard-normal-close.json,
keyboard-after-normal-close.json and keyboard-after-close-later.json.

After the operator confirmed normal App exit, fresh authenticated metadata
confirmed both domains DISCONNECTED and the old GUI process absent. The guarded
stop-if-disarmed check accepted the current cleanup receipts, confirmed resource
release and successful Worker exit; old Host/Worker exit was checked before
launch. No process was force-terminated.

Candidate 03 is now running as GUI 21744 / Host 80984 / Worker 54620, all from
the qualified portable directory with verified executable hashes. The GUI is
responding with a visible window, and the Rust Host is ONLINE under new boot
cdde3233b61d02890e73091a6b1a49c8. Both instrument domains initially remain
DISCONNECTED with no connection. No instrument Connect or output restoration
was issued by the agent. Evidence: keyboard-operator-exit.json,
keyboard-safe-stop.json, keyboard-launch.json, keyboard-new-app-online.json,
keyboard-new-processes.json and keyboard-switch-confirmed.json under
Result/gain-controls. Operator keyboard acceptance on a connected instrument
is separate from the completed offline/browser and package checks.

## Gain 5 Hz sampling and delivery

The requested default is now one native five-field snapshot every 200 ms.
Polling remains serialized, skips missed periods and yields to an already queued
output-off request. Thermal stability still requires five elapsed seconds; an
intermediate out-of-band 200 ms sample invalidates qualification. Moderate
deviation still counts one-second-spaced observations and shuts current off only
on the third accepted observation, avoiding repeated shutdowns between counts.

Worker Gain cache publication and active Host metadata publication target 200 ms.
Host active cache expiry is also 200 ms; due fast publications force metadata
queries to avoid phase beating, and operation completion wakes the idle publisher.
Other device cadences retain their previous values. Chart retention is 4801
actual observations, including a complete 4501-point fifteen-minute window at
5 Hz. Heartbeats do not create points; independent latest-cache phases can omit
intermediate revisions and are not a lossless acquisition channel.

Behavioral RED/GREEN evidence covers native cadence, admitted Off priority,
moderate deviation and five-second stability; Worker first/recurring cadence;
history capacity; and the actual Host cache TTL. Host tests configure only a
metadata Gain draft through the finite OSA fixture and never open a Gain port.
Frozen full regression reports 705 Rust tests and 491 frontend tests passed,
with all 13 packaging assertions passed. The outer command inherited exit 19
from the intentionally failing finite-build packaging case, rather than a test
failure; the separate packaging rerun passes with exception-aware exit 0
accounting (Result/gain-5hz/packaging-green.log).
Mounted Chromium passes 14 bounded scenarios. Independent final source review
reports no P1/P2. Evidence: Result/gain-5hz/full-native.log, host-red.log,
host-green.log; Result/gain-controls/5hz-*-red.log and 5hz-driver-green.log;
Result/gain-failed/gain-5hz-*.log. No hardware throughput or sensor conversion
frequency has been measured by these offline checks.

Candidate 04 was built and qualified before starting the subsequent voltage
investigation: Result/native-package/gain-console-20261009-04/portable, revision
0.1.0-55b53a471de0, source
tree-55b53a471de06619f56fe0bfba2c436f553a9856708e34e4f48272e1fd2cc0b4.
The 29-file / 3-driver / 18-pin contract, AMD64/static CRT checks, exact disarmed
packaged Worker startup and EOF release, and all pristine ZIP entries passed.
ZIP SHA256: 1a99e15bf2fb38d3b49c5f505ece60fe9c660191205b63673514beecce365eec.
Evidence lives in Result/gain-5hz/{build.log,package-check.log,pe,
portable-worker,zip-qualification.json}.

The old candidate-03 GUI accepted normal CloseMainWindow but remained visible
and responding. Authenticated metadata after more than 95 seconds still shows
the same Gain connection and faulted voltage draft responsibility, with no new
matching cleanup receipt. Candidate 04 has not been activated and the old owner
has not been killed or replaced. Evidence: pre-switch-host.json,
normal-close.json, after-normal-close.json and after-close-later.json in
Result/gain-5hz. The voltage repair will be delivered in one successor candidate
after normal release, rather than forcing or duplicating the switch.
