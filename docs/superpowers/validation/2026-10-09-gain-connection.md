# Gain connection fault and recovery

Date: 2026-10-09. Baseline: ef95a06, merge-desktop worktree.

## Observed running version

The old portable package repeat-20261008-01 reports revision
0.1.0-13a75fa6ce81. Its authenticated cached Host inspection records a successful
normal Gain connect followed by an asynchronous driver FAULT. The fault is
`Responsibility("Gain temperature deviation exceeds 3 degC")`. The outer domain
was still READY, with resource responsibility and the connection retained.
Historical temperature 0, target 35, and both output flags off are unknown-quality
cached readings, not current measurements. The failing probe reply was not
retained in the bounded inspection; the exact user-facing normalization branch
is inferred from this state and its reproduced software path.

Evidence: Result/gain-failed/owner-20261009.json. Windows metadata identifies
COM4 as USB VID 10C4 / PID EA60, CP210x; metadata alone is not instrument identity
or evidence of a functioning temperature sensor.

## Driver policy

Normal initialization already sends current-off before TEC-off. The older
unconditional target-deviation watchdog did not account for this idle state:
ambient 22 / target 35 reproduces the same failure as temperature 0 / target 35.
With both outputs off, the watchdog still obtains and strictly parses its entire
snapshot, publishes observed values without substituting a temperature, clears
thermal eligibility and resets deviation continuity. Only target deviation is
inapplicable while neither output is energized.

TEC or current enabled retains the original immediate >3 degC fault and >1 degC
for three consecutive samples current shutdown. Current enable still requires
TEC on and five continuous seconds within target +/-0.2 degC. Protocol errors,
failed reads, malformed/nonfinite fields and current/target limit violations
remain faults even while both outputs are off. Cleanup remains current-off
before TEC-off; resource release and physical output evidence are separate.

The preserved Python driver and historical spec described unconditional target
deviation. They remain historical references; the native both-off idle policy
above is the deliberate compatibility correction for normal native startup.

RED: 3 new behavioral failures among 17 interlock tests. GREEN: all 161
yang-drivers tests passed; the final-format Gain and interlock suites passed
again. Logs: Result/gain-failed/driver-red.log and driver-green.log.

## Worker and wizard

The implementation carries cached Gain health errors through supervised probe
and registration, publishes asynchronous Gain FAULT to domain and registry, and
preserves its connection responsibility until an actual cleanup receipt. These
health checks do not issue instrument I/O. Missing sessions and foreign authority
retain their existing distinct errors.

Connect & verify checks inner driver health as well as domain/control state.
Operator retry of an unhealthy retained connection uses one authority-bound
Host release_control attempt, waits for its exact terminal cleanup ticket and a
fresh synchronized released context, then acquires a new lease and connects once.
Pending, failed or unknown release cannot reconnect; retry checks the original
attempt rather than replaying it. Concrete cached fault text is shown instead of
masking every supervised failure as an unconfirmed connection.

Worker RED: 4 new failures among 13 telemetry tests reproduced READY hiding the
fault, suppressed health/ResourceBusy errors and stale-proof normalization.
GREEN: all 122 Worker tests passed, including no-I/O health/proof, no registration
after fault, healthy/missing/wrong-controller distinctions and retained fault
publication. Logs: Result/gain-failed/worker-red.log and worker-green.log.

Frontend RED / GREEN covered the stale outer READY, foreign ownership, pending
work, wrong ticket/boot/context and uncertain release. Independent review found
three additional edge cases: an edited timed-out draft attempted replacement
before querying its original release; cancellation could reconnect after a
confirmed release; a returned non-completed connect did not persist unknown.
Four behavioral RED failures reproduced these cases before correction. The
original attempt now precedes any draft replacement, cancellation is checked
after release and after acquire, and an unconfirmed connect outcome persists.
The final cross-review found no remaining Important/Critical defects in UI or
Worker. Root separately reviewed the Gain driver and integrated flow.

The mounted production wizard/store was exercised in real headless Chromium
using a finite injected typed Host double with no native/hardware reachability.
Five scenarios passed: delayed release through verified save, cancellation
through the existing operator backdrop, timeout retry of its original ticket,
lost release acceptance without replay, and healthy retained verification without
reconnection. Cancellation produced only release -> cancel, with no new acquire
or startup. Screenshots show pending, unconfirmed and verified states; fault text
appears once. Evidence: ui-red.log, ui-mounted-red.log, ui-review-red.log,
ui-review-green.log, ui-all-green.log and ui-browser-green.log under
Result/gain-failed; screenshots under its browser directory. Browser test source:
App/tests/browser/gain-recovery.mjs. It uses the bundled Playwright module and
installed Chrome through YANG_LAB_PLAYWRIGHT_MODULE and YANG_LAB_BROWSER.

Full offline App/scripts/test-native.ps1 -Offline exited 0: 665 Rust tests,
440 frontend tests and all 13 packaging checks. A subsequent wording regression
adds one frontend test without changing Rust. Final frozen-source frontend is
441 / 441 passing and the 13 packaging checks pass again. Evidence:
Result/gain-failed/offline-final.log, frontend-final.log, packaging-final.log and
offline-summary.json. Existing unused-import/dead-code and test fixture deprecated
fetch_update warnings remain; this change does not alter their unrelated code.

## Concrete candidate

Final portable candidate: Result/native-package/gain-connect-20261009-02/portable.
Revision: 0.1.0-94790e72e716.
Source: tree-94790e72e7164375508cc2876d6fd5a308ddc496a601b37f9ea8a947ac2b145b.

- GUI SHA256: fd6e04cf51c4ebd8833c8f9d4d5542310385c05837e178a416d193fe532df1e5.
- Host SHA256: 870314af8e769f3b82d0603f5313bb682322d6145f615c42937400f990146808.
- Worker SHA256: eb78deda7619e7a372c276428b2f9a0145f7a5d5454b9a702d8d6d016f224952.
- Pristine ZIP SHA256: c87e054ea02cfdaaba31f2fca4b06744e959ecf444caa1d3cb7bdda44f484f7b.

The actual package passed the closed 29-file contract, three driver packages and
18 pinned payload hashes. Every ZIP entry hash matches its pristine portable
file. All three executables are AMD64, with no Python or dynamic MSVC CRT import.
The exact packaged Worker returned its matching revision and real / protocol 3 /
disarmed empty startup state with PATH limited to System32, then exited cleanly
on EOF. This qualification did not start a Host, GUI or instrument. No installer
was built or installed and no clean-Windows/physical qualification is claimed.

An earlier build was rejected by the source-fingerprint guard after the final
wording correction; it produced no staged candidate. The final source was frozen
before the successful build. Evidence: build-invalidated.log, build-final.log,
package-check.log, pe/pe-qualification.json,
portable-worker/9066359a9fc74e80a743c184e1a47d79/qualification.json and
zip-qualification.json under Result/gain-failed.

## Physical boundary

No agent-issued serial/SDK connection, output command, diagnostic, instrument
disconnect or automatic hardware retry has run. The old App retained the Gain
owner until the operator normally disconnected and exited it. Replacement waited
for authenticated cleanup confirmation. No process was force-killed.

The prior scan qualification work is preserved. The production reviewed
single-scan record set remains empty; no offline test establishes actual
Forward/Backward scan speed or stopping behavior. Separately authorized physical
enumeration, read-only and reversible-action stages remain pending.

Cached inspection immediately before requesting the switch still shows Gain
responsibility retained by the old package, while Laser is released. Evidence:
Result/gain-failed/owner-before-switch-20261009.json.

## Confirmed App switch

The operator reported App exit. Windows process metadata confirms the old GUI
gone. Authenticated cached inspection then confirms both domains DISCONNECTED,
null connection IDs, no pending requests, responsibility false, AVAILABLE control
and completed matching normal disconnect receipts. Gain cleanup recorded current
off, TEC off and transport close with no errors or unreleased resources; this is
not an independent physical output measurement.

Only after this evidence, host-inspect --stop-if-disarmed formally stopped the
old Host and confirmed Worker exit / all native resources released. The exact
qualified new GUI was then started. Current GUI PID 69952, Host PID 33396 and
Worker PID 56052 all resolve to the new portable package and match its qualified
SHA256 hashes. Authenticated ping confirms Rust / protocol 3 / verified active
Worker / real Host online, with no startup error. Registry revision 39, saved
Laser identity and the COM4 Gain draft persist. Startup does not restore
instrument connections or outputs; Gain Connect & verify physical retest remains
an operator action after switching.

Evidence under Result/gain-failed: owner-after-exit-20261009.json,
safe-stop-confirmed-20261009.json, app-launch-20261009.json,
new-app-online-20261009.json and switch-qualification-20261009.json.
