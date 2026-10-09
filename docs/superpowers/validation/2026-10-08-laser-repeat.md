# Repeated Goto completion and hold verification

## Incident evidence

The operator reported fault/unusable controls while repeatedly changing
wavelength, then supplied a new moving/waiting screenshot and clarified that
controller readback need not exactly equal the target/actual reading.
Result/laser-repeat/initial-state.json contains the native error
`Tracking-off hold could not be verified; the move was not confirmed complete`.
The error's sole source is a parsed motion read immediately after acknowledged
Tracking Off. The incident's failing sample was not retained, so its particular
failed predicate cannot be asserted. Later cached samples differ from the
1069.410 nm setpoint by -14/+13 pm; they belong to a subsequent connection.
No agent instrument query or setter was used to obtain cached Host evidence.

The previous implementation required actual wavelength within 5 pm both
before and immediately after Tracking Off. Any endpoint, OPC or Tracking
visibility miss after that one read latched a protocol fault. Worker fixtures
made Tracking Off instantaneous and kept wavelength exact; mounted UI fixtures
also modeled exact immediate arrival. Fault snapshots could be displayed as
disconnected while native resources remained owned. A historical
observation_error survived normal release/reconnect.

## Repair contract

The native owner separates arrival, sending Tracking Off and confirming hold.
After ACK it retains an exact target/check-setpoint hold marker. Subsequent
finish calls only read motion until both bracketed OPC observations are complete
and Tracking is off. Valid delayed state is pending, not a protocol fault.
No Tracking Off, target or scan-start setter is replayed. A different endpoint
cannot take over the marker; all motion setters are gated until confirmed hold
or successful explicit Stop. Emission and remote mode retain existing policies.

Goto's application coarse-arrival band is +/-0.02 nm, two motorized-resolution
steps. It is not optical accuracy or a guarantee of sensor variation. The
[manufacturer datasheet](https://www.newport.com/medias/sys_master/images/images/hcd/hd6/9123119824926/SP-NF-DS-20171024-Velocity6700.pdf)
specifies 0.01 nm wide tuning resolution. Setpoint ownership remains exact,
reviewed hardware/operator bounds remain unchanged, and Goto busy completion
still requires two completed observations separated by at least 1 s; time spent
inside a slow getter does not establish dwell. Full Scan retains its original
endpoint prerequisite. Confirmed hold does not require exact endpoint equality.

Worker publishes holding and retained hold_timed_out as active states. A
confirmed hold away from the accepted endpoint is held_off_target with actual
and requested values, without fault, retargeting or a false arrival claim.
Pending hold keeps new moves blocked while Stop remains available. Genuine
transport, malformed and actual operating-range failures retain fault and
resource responsibility. UI distinguishes fault from normal disconnection,
and successful fresh connection/observations clear old observation errors.

## Verification ledger

Native behavioral RED reproduced 14 pm arrival rejection, post-off drift fault,
delayed OPC/Tracking visibility fault and ownership gaps. Additional direct
setter RED proved Target could bypass an unresolved hold. Native GREEN passed
59 driver and 10 protocol tests with finite injected transports.

UI behavioral RED had two failures, then all 13 focused tests and 419 frontend
tests passed. Mounted production UI exercised five repeated Enter commits,
distinct hold verification, non-identical confirmed readback, newer draft
preservation, held_off_target availability and no implicit setter/replay.
This UI sink models observations; native transport tests prove command order.

Evidence: Result/laser-repeat/{initial-state.json,moving-state.json,native-red.log,
native-green.log,ui-red-final.log,ui-green.log,frontend.log,browser.log}.
Worker behavioral RED reproduced five lifecycle/cache defects. A second focused
RED caught external Source change prematurely ending pending hold and a long
Full Scan consuming the hold verification timeout before it began. GREEN passes
all 41 laser tests and all 108 Worker package tests. Hold timeout starts at the
first hold phase, independently of scan duration; external target changes remain
sticky while native hold is unresolved and cannot later be reported as arrival.
Late verified hold after timeout unlocks without a new setter. Explicit Stop and
preserving close keep their separate resource/hold evidence.

Evidence: worker-red.log, worker-hold-red.log, worker-green.log and
worker-package-green.log. Final source is stable; fresh whole-change review and
final full regression are complete. No hardware diagnostics or
physical single-pass qualification have been performed.

Initial whole-workspace regression succeeded with exit 0: 609 Rust tests,
419 frontend tests and 13 packaging groups (offline.log/offline-summary.json).
One fresh whole-change review found no Critical issues, one Important issue
and one Minor test gap. The Important issue was reproduced in two failing
regressions: a full Status observes external Source change, then following
motion sees Source restored; ignoring the first observation could send
Tracking Off after ownership loss or falsely report arrived. The Minor gap
requests short/long Full Scan lifecycle coverage through delayed hold and
off-target confirmation. Both were addressed in one fix pass. Full Status now
latches Source ownership loss before any further conditional Tracking Off;
pending hold retains the flag until verified hold, even when a later read
restores the old Source. New short (3 s) and long (300 s) Full Scan regressions
retain the initial endpoint prerequisite, model delayed Tracking/OPC visibility,
confirm +14 pm held_off_target, count exactly one Tracking Off and no replay,
and verify explicit Goto and Full Scan availability after confirmed hold/Stop.
All 41 laser and 108 Worker tests passed with exit 0. Evidence:
worker-review-red.log, worker-green.log and worker-package-green.log.

The reviewer performed a bounded read-only check and confirmed both existing
findings addressed. Final whole-workspace regression passed with exit 0:
612 Rust tests, 419 frontend tests and 13 package groups. Evidence:
offline-final.log and offline-final-summary.json.

## Actual package qualification

Fresh portable: Result/native-package/repeat-20261008-01/portable.
Package revision: 0.1.0-13a75fa6ce81.
Source revision: tree-13a75fa6ce81484dcee1d6edf0fff2a386e293a158fcac7d1734c0080c9d6cc0.
Actual locked offline build succeeded. Closed package verification passed with
29 regular files, three driver packages, 18 driver pins and no Python/fixtures.
PE inspection confirmed all three executables AMD64 with no Python or dynamic
MSVC CRT imports. The exact hash-pinned packaged Worker, with only System32 on
PATH, reported Rust/protocol 3/startup revision 1, zero domains, disarmed state
and a successful clean EOF exit without capture data or any Host startup.

SHA256:

- GUI: 7205666a5bbb2c91140107bfaea90c0670195a6145c30e8b82c4eba45932eb3f
- Host: 870314af8e769f3b82d0603f5313bb682322d6145f615c42937400f990146808
- Worker: ad32e21a6de9107920e9cc7f514a327a4a0db6368c2614c33392b0fcd8122d9e
- Pristine archive: 968063709656242bc9fb73cafc168deeb7204aa97fd0b8c84c8f6b48c4b916e7

All 29 archive entry hashes match the qualified package. Evidence: build.log,
package-check.json, pe/pe-qualification.json, worker-check.json and archive.json.
The archive was generated before GUI launch. Normal operator release and
verified switch are pending; pre-switch-state.json confirms the previous
native owner still retains the Laser resource after its old hold fault.
No physical arrival/hold or single-pass qualification is claimed.
