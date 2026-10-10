# Gain console implementation plan

> For agentic workers: execute the assigned source scopes in this worktree;
> behavioral RED/GREEN first, then fresh cross-review before integration.

**Goal:** Deliver the operator's Gain dashboard and simple native PID/current
controls while preserving output, authority, cancellation and evidence boundaries.

**Architecture:** Reuse the Rust Gain actor and typed Host/Worker path. Separate
Gain presentation, chart/history and UI action validation into focused modules.
Use a read-only native cache observer during bounded compound operations.

**Tech stack:** Rust, Tauri, plain JavaScript/CSS/SVG; existing Node/browser tests.

**Spec:** docs/superpowers/specs/2026-10-09-gain-console.md.

## Constraints and review focus

- No hardware diagnostics or root-issued output changes for this implementation.
- Driver limits/interlocks/watchdog and current-off-before-TEC-off remain strict.
- No PID defaults or fake chart points; preserve clock/context/quality evidence.
- Off must work during pending normal operations; uncertainty never replays work.
- Live observations must not block on native actions or authorize their completion.
- Dirty target/PID/ramp settings, focus and selection survive updates/navigation.
- Broad normal-control eligibility changes must not affect unrelated instruments.

## Tasks

- [x] Native Gain + Worker: failing generation/stability/deadline/PID/ramp/live
  observation tests; implement bounded start_current and cache observer; wire exact
  action and telemetry contracts; run complete relevant suites.
- [x] Host + catalog: failing strict schema/range/safety validation tests; expose
  read_pid, set_pid, ramp_current, start_current; validate the same contract and
  preserve unsafe/unknown argument rejection; run relevant Host tests.
- [x] Frontend control/history: failing action/draft/authority/safety/gap tests;
  map typed controls, preserve original pending identities, collect timestamped
  bounded observations across routes and expose local gainHistory; full Node run.
- [x] Gain dashboard: failing layout/status/plot behavior tests; extract
  gain-panel.js and gain-trend.js, delegate panels.gain, add scoped CSS; implement
  two top panels and two equal control cards, no routine Fresh/age labels.
- [x] Integration: mounted production browser delayed/error/navigation scenarios
  and visual inspection; cross-review native/frontend and root review Host; fix
  important findings; run final native offline regression and candidate checks.
- [x] Delivery: build exact portable candidate, PE/closed payload/packaged Worker
  qualification and pristine archive; record evidence and commit; switch after
  operator release/exit and authenticate new Host online.

2026-10-09: Candidate gain-console-20261009-01 (0.1.0-b7e9c6dde347) built;
closed payload, PE, disarmed packaged Worker and all pristine ZIP hashes pass.
Final integrated 689 Rust / 480 frontend / 13 packaging and 9 browser groups pass.
The first switch exposed a real Rust-wire JSON property-order comparison defect.
Canonical Host responses reproduced it before correction. Shared exact structural
comparison and clearer Gain recovery presentation now pass 490 frontend tests,
13 packaging checks and 10 canonical browser groups, with clean independent
review. Candidate 02 (0.1.0-bdee06b28b8e) passed the full concrete package checks
and is running as GUI 55048 / Host 17320 / Worker 20144. Authenticated metadata
confirms a fresh ONLINE Rust Host and both instruments DISCONNECTED. Both switches
used normal GUI close guards and verified Host release/exit; no force-kill or
automatic instrument connection/output restoration was issued.
