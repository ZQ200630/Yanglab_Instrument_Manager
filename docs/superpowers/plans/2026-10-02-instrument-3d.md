# Instrument 3D Implementation Plan

> **Historical plan — revised intent, 2026-10-03:** The user clarified that viewers must recognize the instruments; building models or enabling spatial manipulation is not the goal. Read the precedence note in the linked spec before any further work. Do not execute unchecked historical items as new requirements or expand interactive 3D. The proposed compact/static presentation is pending user approval; existing safety and identity constraints remain binding.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render every instrument as an offline interactive mesh with truthful, safe stage state and preview.

**Architecture:** One retained display-only viewer consumes a pure state projection and original mesh builders. Existing Python/IPC safety paths remain unchanged. Vendor CAD replacement remains gated on identity and rights.

**Tech Stack:** Existing static ES modules, Three.js 0.186.1 vendored MIT runtime, Node test runner, Tauri 2, Anaconda VISA.

**Spec:** `docs/superpowers/specs/2026-10-02-instrument-3d-design.md`

## Global Constraints

- Mesh coordinates use millimeters and lab axes +X right, +Y away, +Z up.
- Display-mode scaling is 1x or 1000x, prominently labelled and never applied to command values.
- The view never imports IPC or calls a driver.
- Every Python command runs in Anaconda VISA. No real instrument I/O.
- Custom boards and unconfirmed holders/layout are marked provisional.
- The existing strict Tauri CSP stays unchanged.

## Review Focus

- Lost role ownership, stale status and wrong side serial must not leave an apparently valid pose.
- Redraw during pointer use must preserve the canvas/context/camera, not create new WebGL contexts.
- Renderer/context failure must not crash instrument controls or replace unknown pose with claimed zero.
- Arbitrary/nonfinite preview and scale inputs must not create misleading movement or hardware writes.
- A manually adjustable micrometer must stay fixed when a piezo command changes the estimated deck.

### Task 1: Offline meshes and truthful state

**Files:** Create `App/web/scene/{catalog,state,models}.js`, `App/web/vendor/three/*`, `App/tests/scene.test.mjs`.

**Interfaces:** `sceneState(page,state,drafts={},scale=1)` returns `{kind,stages,scale,caption}`. Each stage carries side, known, positionMm, previewMm, label. `buildModel(kind)` returns `{root,stages,chip}`; stage entries expose `{deck,origin}` and all geometry/materials are owned by root.

- [ ] Write tests first: unknown connected state yields no position; serial mismatch/fault invalidates; left +0.1 um maps to +0.0001 mm (at 1x) and +0.1 mm (1000x); no preview beyond +0.2 toward-chip; model's deck changes without moving chip or knobs.
```js
assert.deepEqual(sceneState('fiber', authorizedLeft, {}, 1).stages.left.positionMm, [0.0001,0,0]);
assert.equal(sceneState('fiber', wrongSerial).stages.left.known, false);
```
- [ ] Run `node --test App/tests/scene.test.mjs`; expected missing-export assertion failure.
- [ ] Vendor official runtime after package SHA512 verification. Implement pure validation using registered serials and `previewStageMove`, and named mesh groups with distinct mechanical parts, instrument screens/connectors/feet/knobs. Add provenance descriptions.
```js
deck.position.set(origin.x + delta[0], origin.y + delta[1], origin.z + delta[2]);
```
- [ ] Run scene tests then `node --test App/tests/*.test.mjs`; expected all pass. Commit scoped files.

### Task 2: Retained renderer and panel

**Files:** Create `App/web/scene/{viewer,panel}.js`, `App/tests/scene-viewer.test.mjs`; modify `App/web/style.css`.

**Interfaces:** `createViewer(host,{onError})` returns `{update(view),rotate(key),reset(),dispose()}`. `createScenePanel()` returns `{element,update(page,state,drafts),preview(drafts),dispose()}`. No dependency receives a worker or action callback.

- [ ] Add tests for repeated updates retaining context, disposing mesh resources, camera keys not changing pose data, stale pose hiding preview, and throwing renderer initialization producing fallback text while callers can continue.
```js
const before = snapshot.stages.left.positionMm.slice();
viewer.rotate('ArrowRight');
assert.deepEqual(snapshot.stages.left.positionMm, before);
```
- [ ] Run `node --test App/tests/scene-viewer.test.mjs`; expected missing capability failure.
- [ ] Implement one on-demand renderer, Z-up camera, limited orbit/zoom, labelled lab arrows, panel controls, resize handling, WebGL-loss fallback and idempotent disposal. Default scale 1x; explicit toggle 1000x.
- [ ] Run all Node tests; expected all pass. Commit scoped files.

### Task 3: Console integration and verified visuals

**Files:** Modify `App/web/main.js`, `panels.js`, `style.css`, relevant UI tests, `App/README.md`, `docs/acceptance.md`.

**Interfaces:** `render()` retains the panel DOM node across content redraw, then `scenePanel.update(state.page,state,drafts)`. `updateStagePreview()` passes draft values without action invocation. Dedicated paired fiber model replaces the prior CSS stage graphics, preserving accessible directional text.

- [ ] Write failing production-main regression asserting initial offline overview has scene, each device page selects its model without worker start/action, and draft input changes preview only. Test canvas panel retention through polling; update intentional old-CSS tests to new accessible view behavior.
- [ ] Run main/UI tests; expected new integration assertions fail.
- [ ] Integrate retained panel without changing command payloads or confirmations. Remove obsolete CSS motion handlers. Keep camera input isolated from form fields. Show geometry confidence and unknown status.
- [ ] Run `node --test App/tests/*.test.mjs`, VISA `python -B -m unittest discover -s App/tests`, and original offline suite. Expected all pass; any failure is investigated, not hidden.
- [ ] Start simulation-only `python -B -m App.tests.preview_server --port 8766` in VISA; inspect rendered device pages and paired-stage movement/preview at 1440x900 and 960x680. Stop server after QA. No real hardware.
- [ ] Build `cargo tauri build --ci -- --locked` with installed MSVC/SDK and D-local caches. Verify runtime/model assets included. Record native UI evidence separately from browser evidence. Commit and request one fresh whole-change review before delivery.
