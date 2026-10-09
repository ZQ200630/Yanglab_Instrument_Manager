# Gain connection failure and retained-fault recovery

Date: 2026-10-09. Baseline: ef95a06, merge-desktop worktree.

Observed old package repeat-20261008-01 connected Gain COM4 successfully, then
its asynchronous driver faulted: cached T=0, target=35, current and TEC off.
The values are historical, unknown-quality observations, not fresh sensor proof.
The normal initialization added current-off then TEC-off without updating the
older unconditional target-deviation watchdog policy. Valid ambient 22 / target
35 also reproduces the idle fault. The physical meaning of a zero readback is
unknown and this change does not treat it as a verified working sensor.

Root causes and bounded corrections:

- [x] Driver: a both-outputs-off session still reads/parses its watchdog snapshot,
  resets thermal stability and deviation continuity, but target deviation is not
  an energized-output alarm. TEC/current enabled retains the original >3 immediate
  and >1 for three samples thresholds; current still requires TEC and five seconds
  within target +/-0.2. Every protocol/transport/field-limit failure remains a fault.
  Shutdown always attempts current off before TEC off.
- [x] Worker: publish asynchronous Gain FAULT to domain/registry instead of stale
  READY while preserving connection and responsibility. Retained supervised health
  failures retain their concrete cached fault; missing/busy/closed sessions remain
  explicit. Health checks and proof use no driver I/O or connection replay.
- [x] UI: a driver FAULT is never a healthy retained connection. One operator
  Connect & verify action can release its unhealthy old owner using the existing
  authority-bound release API, await fresh matching terminal release evidence,
  obtain a new lease and connect once. Failed/unknown/pending release does not
  reconnect or repeat release. Healthy sessions still reuse their connection.
  Show the useful fault and retained/release state, not a generic masked message.
- [x] Verify each behavioral RED / GREEN, perform one final independent review,
  run full offline native/frontend/packaging suite, mounted wizard check and build
  a concrete portable candidate. Record actual results and preserve all existing
  scan qualification fences.
- [x] Switch only after the operator normally releases current instruments and
  exits old App; confirm authenticated release receipts before formal Host stop.
  Do not force-kill, reopen COM4, restore output or perform an implicit hardware test.

Implementation approval does not authorize serial/SDK diagnostics or output
changes. Existing live App is left running until explicit normal release. The
single-scan physical stages from the preceding task remain unexecuted.
