# Desktop branch integration

## Goal
Merge codex/pic-desktop (14e171a74403317e7a4adb0208270f30c3ea1c6d) and codex/laser-1060-desktop (f65398258a46c64a946b240d302ea0f0bb9f754f) with both histories and current behavior preserved. Publish the tested two-parent integration to main. Keep both development branches and the currently running application intact.

## Architecture
Use PIC's native Rust Worker, transports, lifecycle, capture recovery, packaging, and remote support as the production foundation. Connect the Laser branch's existing Rust TLB bus to native Worker sessions, and preserve its UI, driver packages, fixed installer, and configuration activation. Python source remains a historical/reference/offline surface, never the active application backend.

## Tech Stack
Rust workspace (MSVC), Tauri Host and static JavaScript frontend, native Windows SDK/SetupAPI, existing finite transport fixtures. All Python invocations must use D:/Program/Anaconda3/envs/VISA/python.exe.

## Spec
User requested merging PIC and Laser development branches. Existing branch specifications and project AGENTS.md are binding behavior and safety sources. Driver settings must retain direct Install for missing drivers and compact readiness rows; startup never prompts to install. Manufacturer public redistribution authorization already supplied for CP210x.

## Global Constraints
- No hardware enumeration, connection, output changes, installer execution, or termination/replacement of user-owned Host in this task. Use finite injected fixtures only.
- Preserve all project voltage, gain, PM400, MDT, fiber, and Laser safety/admission/cleanup constraints. Laser connection and ordinary close preserve outputs/settings; never kill unresolved SDK ownership.
- Production GUI/Host uses native Worker without Python. Keep native Worker provenance checks, finite wire validation, configuration revisions, remote sessions, original capture recovery, and package allowlists.
- Preserve exact vendor driver bytes and packages.json size/hash pins. USB serial drivers expose ready/missing/not_detected/unavailable states; missing requires present Windows problem 28, ready requires supported service with problem 0. No-device/unknown must not permit USB serial install. Newport preserves the Laser branch prerequisite rule: a separately verified absent SDK may permit explicit manual package installation without an attached Newport device; unknown SDK/metadata never permits install, and all ownership/disconnection/pin checks remain mandatory.
- CH340 identity: VID 1A86 PID 7523/5523; CP210x: VID 10C4 PID EA60/EA63/EA70/EA71/EA7A/EA7B. Skip USBCCGP composite parents; evaluate interfaces.
- Full Node and native offline suites plus bounded Laser and metadata contract tests must pass before commit/push. Review semantic auto-merges as well as conflict markers.
- Git identity and repository trust are command-scoped. One final merge commit must have both original development tips as parents. No force-push or source branch deletion.

## Review Focus
Laser registry limits must reach the native driver and actions/readbacks must preserve acknowledgement versus completion. USB metadata must remain read-only and separate from SDK enumeration. Packaging must contain all driver resources and native binaries without resurrecting Python. Frontend must retain remote and setup flows, selected-device checks, and Settings Install.

## Tasks
1. [x] Resolve frontend and its test conflicts, preserving both branches' user flows. Own App/web and App/tests/*.mjs only.
2. [x] Integrate Rust TLB driver into native Worker sessions and finite tests. Own native Worker except discovery and USB metadata, TLB crate, root Cargo.toml/Cargo.lock, and native worker fixture changes required for Laser.
3. [x] Add native read-only driver inventory metadata and finite tests. Own App/worker-rs/src/discovery.rs and new metadata module in Code/Utils plus corresponding tests/Cargo Windows features; coordinate shared manifest edits.
4. [x] Resolve Host/runtime/build/package/docs integration; review auto-merges for native production consistency. Own remaining conflicts and integration tests/documentation. Do not alter frontend/native driver ownership without coordination.
5. [x] Run combined full offline verification, package validation and broad final review; resolve findings and publish normal merge to main after checking remote hasn't moved.

## Decisions
The managed worktree tool fails the repository ownership check; persistent global trust was rejected by automatic approval. Use command-scoped safe.directory with an ignored project-local worktree. Source branch tips remain unchanged.

## User clarification
The merged software must run wholly on the Rust application/backend path. No Python backend, launcher, fallback, interpreter setting, or runtime payload may remain in the production app/package. Existing historical Python reference/offline source is not a backend and must be clearly marked, excluded from packaging and not invoked by the app.
