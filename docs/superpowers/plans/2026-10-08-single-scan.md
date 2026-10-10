# Complete qualified Forward / Backward scanning

Intent: the operator already selected four actions. Full Scan is one round
trip; Forward and Backward start at the current position and hold at the
selected endpoint; Stop holds current position. Default rate remains 0.1 nm/s.
User approved completing the existing single-pass qualification/enabling logic.

The existing production capability is always false, and ErasedWire loses the
transport capability. Existing RESET candidate has typed bounds but the manual
does not establish its actual slew contract. The old diagnostic cannot qualify
speed and can claim hold with Tracking still on. No real qualification exists.

Implementation preserves one native owner, real-only startup, preserving close,
the approved head/operator bounds, exact identity, explicit physical stages and
no setter replay. No general runtime override or automatic hardware test is added.

## Work

- [x] Native qualification module and compiled reviewed records, bound to exact
  controller/firmware/head identity, actually loaded SDK SHA256, protocol revision
  and individually verified rates. Empty records give unsupported; malformed,
  duplicate or mismatched records fail closed. Source fingerprint includes data.
- [x] Native status exposes verified rates. Single Scan rejects an unqualified
  rate before setters and joins typed BeginMove for inherited Tracking takeover.
  Fresh naturally complete endpoint with Tracking off is Held, with no extra
  setter; off-target/external movement remains truthful.
- [x] Worker forwards capability and rates, publishes them to the UI and owns
  single moves through the shared BeginMove/finish/Stop lifecycle. Finite tests
  exercise both directions, qualified/unqualified identity/rate, natural hold,
  delayed hold, repeated moves and uncertain result without replay.
- [x] Diagnostic trajectory has read start/end times, direction and enough
  intermediate displacement to evaluate 0.05 / 0.1 nm/s conservatively. First
  sample at endpoint is insufficient speed evidence. Hold requires separate
  observations spanning a second, OPC complete, Tracking off and coarse position
  stability. Output/SCANCFG readbacks remain unchanged; no optical claim.
- [x] Hardware-free candidate reviewer requires six successful real reports:
  endpoint in both directions at both rates, and midway Stop in both directions
  at 0.1. It recomputes predicates, requires same exact identity/SDK and clean
  release, records six immutable report hashes, and never installs a qualification.
- [x] UI shows the pending reason or verified rate list. Qualified supported
  actions use the verified rate for that direction; other velocity values cannot
  be admitted by a capability boolean alone. Draft, Stop, emission and navigation
  remain independent of conflicting motion gates.
- [x] Run full native regression and one fresh review; fix findings in one pass.
  Build/qualify a concrete candidate and preview diagnostic plans before asking
  for separately explicit enumeration, read-only and reversible-action consent.
- [ ] With physical authorization, release current owner normally, execute each
  authorized stage, review evidence, compile successful qualification only if all
  predicates hold, qualify final package and switch. Insufficient/failed physical
  evidence does not enable a production capability or cause command replay.

## Physical acceptance boundary

Use bounded 0.20 nm probes, requested 0.05 / 0.1 nm/s, all inside current limits.
RESET's possible rate may be the head maximum (10 nm/s for 6722-P); disclose that
uncertainty before any action consent. Do not automatically return to origin or
change emission/blanking. Midway Stop/timeout Stop are explicitly part of the
approved probe. Controller readback qualification is not independent optical
speed, wavelength accuracy or blanking verification.

Approval for implementation does not substitute for the three separately
authorized hardware stages in AGENTS.md. The current App owns the Laser; it must
release normally before any exclusive diagnostic owner is acquired.
