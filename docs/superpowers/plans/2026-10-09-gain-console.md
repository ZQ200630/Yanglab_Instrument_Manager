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
- [ ] Delivery: build exact portable candidate, PE/closed payload/packaged Worker
  qualification and pristine archive; record evidence and commit; switch after
  operator release/exit and authenticate new Host online.

2026-10-09: Candidate gain-console-20261009-01 (0.1.0-b7e9c6dde347) built;
closed payload, PE, disarmed packaged Worker and all pristine ZIP hashes pass.
Final integrated 689 Rust / 480 frontend / 13 packaging and 9 browser groups pass.
Delivery remains pending normal operator disconnect/exit: authenticated metadata
still shows the Gain connection retained by the running older package.
