# Laser control Implementation Plan

> Execute inline with superpowers:executing-plans and test-driven-development. The existing user-selected codex/laser-1060-desktop branch is the workspace.

**Goal:** Simplify laser control and add manufacturer-native scanning with automatic model limits.
**Architecture:** Rust owns serialized typed composites; Python exposes reusable driver methods; Host/catalog validate intents; UI renders readback and submits asynchronous typed operations.
**Tech stack:** Rust, Python 3.10 VISA, vanilla JS, Tauri.
**Spec:** docs/superpowers/specs/2026-10-07-laser-control.md

## Task 1: Driver and contracts
- Write bounded native and Python tests for model/P recognition, strict unknown suffixes, input rejection, scoped Remote/Local, wavelength tracking, scan ordering/readback, speed caps, busy stop, and no replay after failure.
- Run tests. Expected: new interfaces/recognition fail before implementation.
- Implement typed controls and strict native/worker/Host/catalog schemas.
- Run native Rust and Python driver/App tests. Expected: all pass.

## Task 2: Interface
- Write UI tests for compact status, one output action, large controls, scanning intents, collapsed fine tuning and bottom identity.
- Run tests. Expected: old UI fails the new assertions.
- Implement layout and bounded foreground refresh, preserve input state and asynchronous progress.
- Run Node tests and isolated Edge acceptance. Expected: all pass without device access.

## Task 3: Delivery
- Review changed layers for fault, authority, manual-control and scanning behavior.
- Run appropriate offline suites, build native/Host/desktop, verify package, preserving disconnect/update using existing authorization.
- Record validation and limitations; commit/push the approved branch.

## Review focus
Partial setting failure must never start a scan. Return-Local failure must not replay the original action. STOP must be accepted while busy. Unknown-head tuning remains blocked. UI must not invent emission or scan completion. Polling must not overlap requests or interfere with another page.

## Completion evidence

- [x] Driver/model/composite/scan contracts and both layers of limits implemented and verified.
- [x] Compact status, primary control layout, collapsed advanced controls, preserving inputs and async progress verified in Node and isolated Edge.
- [x] Limit persistence and activation review fixes verified, including native Host reconnect at the saved revision.
- [x] Full offline suites, native/Host/desktop build and 42-resource package check passed.
- [x] Authorized real read-only two-sample qualification and preserving release passed; updated GUI reopened with the original registration.
- [x] Validation and hardware-action limitations recorded in docs/superpowers/validation/2026-10-07-laser-control.md.

The checks above describe the first delivered revision. Later steering adds digit editing, independent return velocity and latency changes; its final validation is recorded separately below.

## Latest steering: Ready target and latency

- [x] Fixed aligned target digits: left/right select, up/down adjust; bounded latest-value queue with no action replay or cross-context reuse.
- [x] Target writes preserve Tracking/Ready, scanning disables Target and Stop adopts actual readback. Independent forward/backward velocity is validated against both envelopes.
- [x] Target/Piezo/Start/Stop ACK completes separately from five-getter motion observation. Full power/current/output acquisition timestamps remain independent.
- [x] Initial allowlisted getter pacing 10 ms, retained 200 ms getter retry and setter pacing. Existing SDK framing/drain/one-getter-retry and no-setter-replay fences remain.
- [x] Final offline suites/review/build/package and real read-only repeated timing qualification.
- [ ] Preserving desktop update, final evidence, commit/push.

No output/tuning/scanning hardware diagnostic is authorized by the read-only timing run. User-configured limits apply to all commanded scan legs and cannot exceed the automatic hardware envelope.
