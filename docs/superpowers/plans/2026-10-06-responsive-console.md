# Responsive Console Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans inline. One independent review after all changes.

**Goal:** Remove avoidable interaction stalls and preserve live controls during updates.

**Architecture:** Incrementally patch same-page DOM; coalesce background updates with immediate action rendering. Separate refresh from policy persistence.

**Tech Stack:** Existing JavaScript, Node tests, isolated headless Edge browser acceptance; no new runtime dependency.

**Spec:** `docs/superpowers/specs/2026-10-06-responsive-console.md`.

## Global Constraints

- Retain all Host/context/lease/safety gates and native SDK pacing.
- Real instrument diagnostics are read-only, staged and use `Code/Debugs`.
- Python commands use the Anaconda VISA environment.
- No additional confirmation flows or selectable fake instrument backend.

## Review Focus

- Open native selectors and focused edited inputs during telemetry.
- Eligibility attributes change immediately on faults or ownership loss.
- A queued background render cannot undo immediate action progress.
- Refresh must preserve unsaved interval edits and not authorize new probes.
- Late requests, navigation and release uncertainty retain existing fences.

### Task 1: Preserve live DOM and responsive rendering

Files: add `App/web/render.js`; modify `App/web/console-ui.js`, `App/web/main.js`, `App/web/style.css`; add `App/tests/render.test.mjs` and isolated `App/tests/browser/responsiveness.mjs`.

- [x] Write failing real-browser acceptance for input/select node identity, focus, draft value, expanded details, scroll and fault eligibility during telemetry; run and record RED.
- [x] Write failing scheduler tests for burst coalescing, immediate flush and cancellation; run RED.
- [x] Implement `patchMarkup(target,previous,next)` and `createRenderScheduler(render,schedule,cancel)`. Navigation replaces the page; telemetry preserves compatible nodes. Export the existing `replaceMarkup` API using the new patcher.
- [x] Wire background session changes to `requestRender`, action rendering to immediate flush; add a CSS pending indicator respecting reduced motion.
- [x] Run Node suite and browser acceptance; record GREEN.

### Task 2: Remove redundant refresh work

Files: modify `App/web/console-ui.js`, `App/tests/connection.test.mjs`.

Additional observed root cause: pending button keys lacked an instrument/page scope and disabled another same-model instrument. Add scoped keys, pinned by a deferred-connect navigation test.

- [x] Add failing click-handler test: Refresh now invokes refresh exactly once and resync once, never saves policy, retains draft interval; deferred refresh immediately displays progress, suppresses duplicates and permits navigation.
- [x] Add failing Save interval test: policy save plus one resync, no instrument refresh.
- [x] Split the two action branches and run Node suite plus browser acceptance.

### Task 3: Verify, review and deliver

- [x] Run relevant offline regression, independent review and one fix pass if needed.
- [x] Confirm real Host disconnected and controls available before normal GUI close/Host stop; build and reopen the concrete package.
- [x] Verify packaged resources and healthy Host startup. Record limits and evidence in `docs/superpowers/validation/2026-10-06-responsive-console.md`.
- [x] Commit and push the user-assigned branch under existing session authorization.
