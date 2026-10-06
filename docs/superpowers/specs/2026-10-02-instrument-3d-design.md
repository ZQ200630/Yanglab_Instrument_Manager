# Instrument 3D display design

## User correction — 2026-10-03 (takes precedence)

The user clarified: "我的目的不是让你把模型建立出来。我的目的是让别人能看出来是什么仪器。所以也不需要说我可以在三维空间内操控". The outcome is recognizable instrument identity, not model construction or spatial interaction. Treat the interactive-3D requirements below as the historical implementation approach, not a mandate to expand orbit/zoom, motion animation or CAD work.

Acceptance must assess whether another operator can associate the displayed instrument with the real device; mesh counts, camera controls and passing renderer tests cannot establish that. Keep instrument operation, readouts and the safety/identity boundaries separate from decorative appearance. User-provided left/right stage identity and lab-axis directions remain valid. Do not invent a custom-device enclosure or call the current placeholder boards photo-matched.

A compact static image/fixed-view presentation was proposed in chat, but has not yet been approved. No display replacement is authorized by this note. The current code is unchanged; asset/reference audit found procedural standard-instrument meshes and explicitly provisional custom boards, but no device photos in the searched repository assets. Await the user's response before implementing the proposed visual simplification. Driver limits and staged real-hardware authorization are unaffected.

## Intent and authority

The operator wants every device to be recognizable as a physical instrument, especially the two NanoMax stages, rather than a labeled box or a CSS diagram. This expands the console's display subsystem, not its instrument-control authority. The user's standing instruction is to make reasonable design decisions and continue for later acceptance; this document records those decisions without claiming the unprovided setup photos are known.

## Approach

Use real local Three.js meshes with lighting, camera orbit/zoom, lab-coordinate axes, and a joint view of both stages and the chip. Manufacturer CAD can replace individual model builders once exact identity and redistribution rights are established. The initial original procedural models have separate stage body/deck/holder/micrometer parts and recognizable instrument faceplates; each is explicitly approximate, not vendor CAD. Custom boards and unconfirmed holders/layout are marked provisional. A flat CSS scene is insufficient; high-poly photoreal rendering is unnecessary for responsive control. Meshes and runtime are offline, without CDN requests or a new Node build requirement.

## Model and evidence boundaries

Cover NanoMax MAX312D left/right, MDT693B left/right, AQ6370-series OSA, PM400 console, custom eight-channel Voltage Source, and Gain Driver. The actual PM400 probe and custom hardware geometry require operator identification/photos and must not be invented as exact models. OSA E-series exterior drawings are a family reference, not proof of the connected suffix. Device labels show geometry provenance, connection status and the difference between software observation and physical measurement. Factory button/LED shapes are decorative unless backed by an explicit readback; no fake active lights or front-panel state.

Mesh coordinates use millimeters and lab axes +X right, +Y away, +Z up. Left/right model placement is schematic until a bench photo or measurements establish spacing and orientation. Left +X and right -X point toward the fixed chip. Logical axis signs are not changed by rotating a model for appearance. The stage deck/holder moves, not the chip or manual micrometer knobs. Manual coarse position is not inferred from piezo voltage.

## State and interaction

The view is available without starting a worker or connecting instruments. It never imports IPC or calls a driver. Pointer drag/orbit, wheel zoom, keyboard arrows, Home/reset, model selection and 1x/1000x displacement display only change visualization. Existing typed controls and explicit confirmations remain the only execution path. The renderer accepts immutable view data and separately labelled unexecuted move previews. A preview is shown only for a valid, authorized current-stage estimate and an allowed nonzero per-axis move; no animation claims execution before returned status.

Position is a session-only open-loop estimate. Require connected, error-free Fiber role, matching registered serial, available unrestricted/fault-free side, adopted baseline, nominal authorization and finite coordinates before displaying it. Lost authority hides the estimate/preview; geometry returns to a clearly labelled neutral reference pose, not zero voltage or known origin. Display-mode scaling is 1x or 1000x, prominently labelled and never applied to command values. Keep the chip fixed. No collision clearance or measured contact claim is derived from the schematic scene.

## Architecture and lifecycle

`App/web/scene/state.js` derives data without DOM/GPU/IPC. `models.js` constructs named, independently movable meshes; `catalog.js` carries model provenance. `viewer.js` owns one WebGL context, camera, scene, event handlers, resize observation and resource disposal. `panel.js` owns accessible controls, fallback text and host attachment. The console retains the same viewer element across polling redraws, preserves camera and display preferences, and only rebuilds geometry when the displayed model changes. Render on demand; no perpetual animation loop. Cap pixel ratio at 2. Dispose geometries/materials/textures/observers/listeners on final teardown, hide on settings, and show a non-blocking fallback if WebGL fails or loses context. Instrument controls remain functional.

Only local, pinned, MIT-licensed Three.js runtime files are bundled. Record version, upstream URL, package integrity and file hashes. No downloaded vendor asset is included without a recorded compatible license. The existing strict Tauri CSP stays unchanged.

## Verification and remaining acceptance

Unit tests exercise model hierarchy, correct lab-coordinate motion, fixed chip, unknown/fault/serial mismatch handling, preview limits, scale isolation, camera-only controls, resource disposal and unavailable WebGL. Existing frontend and VISA Python suites must remain green. Browser QA must show genuine rendered geometry on every device page, rotated stage views, unexecuted preview vs returned estimate, keyboard controls, no new hardware requests, and stable repeated status polling. Build the native bundle and verify offline asset inclusion. Native WebView2 interaction and physical diagnostics remain separately gated; an approximate mesh is not final photo-matched acceptance.

## Sources / confidence

- Three.js local installation: https://threejs.org/manual/en/installation.html ; MIT license https://github.com/mrdoob/three.js/blob/dev/LICENSE . Pin 0.186.1 from official npm metadata (2026-10-02).
- MDT693B official CAD list: https://www.thorlabs.de/thorproduct.cfm?partnumber=MDT693B . Exact CAD listed; redistribution rights unresolved, not bundled.
- MAX312D official family page: https://www.thorlabs.de/newgrouppage9.cfm?objectgroup_id=2386&pn=MAX312D%2FM . Exact geometry file not acquired.
- PM400 official specifications: https://www.thorlabs.com/newgrouppage9.cfm?objectgroup_id=3328&partnumber=CAL-PD . Console dimensions 136 x 96 x 29.5 mm; probe unknown.
- AQ6370E exterior drawing: https://cdn.tmi.yokogawa.com/1/9764/files/SDAQ6370E-01EN.pdf . Family reference only, suffix pending identity verification.
