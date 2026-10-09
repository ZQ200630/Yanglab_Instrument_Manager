# Repeated Laser Goto completion repair

> Execute with the existing isolated native Rust worktree and bounded offline transports.

**Goal:** Repeated Enter/Goto completes without converting ordinary controller
readback variation or delayed Tracking Off visibility into a connection fault.

**Approved semantics:** The operator edits a local draft, commits with Enter or
Goto, and expects tracking off after arrival. The latest operator clarification
explicitly rejects requiring exact equality between setpoint and readback.
One accepted move remains exclusive; Stop and emission stay independent.

**Evidence:** Result/laser-repeat/initial-state.json contains the exact native
Tracking-off verification error. Code proves it is emitted after a successful
Tracking Off ACK and one parsed motion read. That error discards the failed
sample, so the incident cannot distinguish endpoint drift, OPC lag or Tracking
visibility lag. Cached subsequent samples differ from target by -14/+13 pm;
they are not a trace of the failing exchange. Fixture replies assumed immediate
OPC=true, Tracking=false and exact wavelength, missing all three real conditions.

**Architecture:** Keep one serialized native owner. Separate arrival, sending
Tracking Off, and read-only hold verification. Native per-session hold ownership
records the authorized target only after ACK, then reads until completed with
Tracking off, without replay. Worker publishes holding as an active phase.
Confirmed hold outside the arrival band reports held_off_target, remains usable,
and never invents an exact arrival or automatically retargets. Hard transport,
malformed and actual operating-range violations remain faults.

**Arrival policy:** Goto uses an application acceptance band of +/-0.02 nm,
two published motorized-resolution steps; it is not optical accuracy or a
manufacturer sensor-noise guarantee. Source setpoint ownership remains exact.
Goto's two observations separated by at least 1 s remain mandatory when OPC is
busy; getter duration does not count. Full Scan retains its existing endpoint
prerequisite; post-Off hold verification is independent of endpoint equality.
The primary manufacturer datasheet gives 0.01 nm wide tuning resolution:
https://www.newport.com/medias/sys_master/images/images/hcd/hd6/9123119824926/SP-NF-DS-20171024-Velocity6700.pdf

**Constraints:** No agent instrument commands, diagnostics or forced process
termination. No new dependencies, Python runtime, output-policy changes,
automatic replay or physical single-pass qualification claim.

- [x] Native RED/GREEN for drift, delayed OPC/Tracking visibility, hard failure,
  exact ownership, Stop cancellation and independent controllers.
- [x] Worker RED/GREEN for multiple Gotos, holding phase, truthful completion,
  timeout/no replay, retained hard faults and fresh cache after reconnect.
- [x] UI RED/GREEN and mounted browser for holding, draft preservation, queued
  Stop, restored controls after confirmed hold and truthful connection faults.
- [x] One fresh whole-change review; address findings in one pass. Full native
  regression, actual fresh portable build, closed package/PE/private Worker
  verification and pristine archive before operator normal-release switch.
- [ ] Confirm operator normal release, formally stop the disarmed old Host,
  start the qualified GUI and verify automatic native Host startup.

**Review focus:** ACK never means held; a known parsed transient is not a
protocol fault; no second Tracking Off during hold polling; timeout never makes
an unresolved hold available for new Goto; external setpoint changes do not
reconcile themselves; cached historical errors cannot describe a new session.

## Ledger

- Native stable: 59 driver + 10 protocol GREEN, including direct setters that
  previously bypassed pending hold. Ordinary preserving reads unchanged.
- Worker stable: 41 laser tests and all 108 Worker package tests GREEN. Follow-up
  RED covered external Source change during pending hold and long Full Scan
  consuming the hold timeout. Hold timeout now starts at hold_started, and
  target_changed stays sticky until verified hold or explicit operator Stop.
- UI stable: two behavioral RED failures then 13 focused and 419 frontend
  GREEN. Mounted UI includes five repeated commits with non-identical readback
  and preservation of the next draft during holding.
- One fresh whole-change review completed. No agent instrument command was issued.
- Initial full regression succeeded, exit 0: 609 Rust / 419 frontend / 13
  packaging groups. Fresh review found no Critical issues, one Important full
  Status ownership blind spot and one Minor Full Scan lifecycle test gap.
  Both ownership cases reproduced RED. One fix pass now latches full Status
  ownership evidence before conditional Tracking Off, and keeps pending hold
  ownership loss sticky through verified hold. Short (3 s) and long (300 s)
  Full Scan regressions cover delayed Tracking/OPC visibility and +14 pm
  confirmed off-target hold, with exactly one Tracking Off and no new movement.
  The reviewer confirmed both findings addressed with a bounded read-only check.
  Final full regression succeeded, exit 0: 612 Rust / 419 frontend / 13 package
  groups (offline-final.log and offline-final-summary.json). Fresh portable
  0.1.0-13a75fa6ce81 passed build, closed 29-file checks, AMD64/no Python/no
  dynamic CRT import checks and exact disarmed Worker ping/clean EOF checks.
  Pristine archive has all 29 entry hashes verified. Normal operator release
  and switch remain pending.
