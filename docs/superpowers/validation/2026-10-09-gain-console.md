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
