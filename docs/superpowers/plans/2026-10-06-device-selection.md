# Device selection and direct actions implementation plan

> **For agentic workers:** Use superpowers:executing-plans for inline execution of the user's bounded changes. Review the whole branch after verification.

**Goal:** Choose TLB controllers by detected serial and keep routine operations free of Yes/No dialogs.

**Architecture:** Keep authenticated Rust Host admission and the existing driver boundaries. Add a read-only controller scan and a serialized, named installer maintenance operation. The frontend owns transient form/scan states and expresses operator intent through named buttons and inline attestations.

**Tech Stack:** Tauri, Rust, browser JavaScript, current VISA Python worker.

**Spec:** `docs/superpowers/specs/2026-10-06-device-selection-design.md`

## Global constraints

- No raw instrument commands or bypass of driver limits, identity, cleanup or ownership gates.
- All Python runs use the Anaconda VISA environment.
- No hardware writes, laser enabling or installer execution during this verification.
- Routine success produces no driver banner; Windows UAC remains a system requirement.

## Review focus

- Late scan responses after profile changes cannot populate another form.
- Multiple controllers and removed/reordered devices cannot change the chosen serial silently.
- Installation and connection admission cannot overlap across GUI clients.
- Reboot-required and cancelled/failed installation cannot be presented as readiness.
- Fiber attestations cannot survive authority, connection or fault changes.

### Task 1: Direct actions

Files: `App/web/console-ui.js`, `setup-actions.js`, `setup.js`, `panels.js`, `pm400.js`; tests in `App/tests`.

- [x] Add regressions for named Connect/Test actions without browser confirmation, inline effects, and independent scoped Fiber attestations; run to observe failure.
- [x] Remove production confirm/prompt calls; use inline forms for names/setup members, scoped baseline checkboxes, and retain prepare tokens/typed consent arguments.
- [x] Run the affected Node tests.

### Task 2: Controller selection

Files: `App/web/setup-actions.js`, `setup.js`, `host-client.js`, `App/worker/discovery.py`, `controller.py`, `contracts_v3.py`, Rust `runtime.rs`, `gui.rs`, `host/service.rs`.

Interface: `scanLasers()` calls named `scan_lasers` with empty params and returns `{controllers:[{device_key,serial}]}`; enumeration emits only fresh identity-derived keys.

- [x] Add regressions for two devices, empty/duplicate identities, disappearing selected serial and stale replies; observe failure.
- [x] Implement bounded read-only enumeration through the reserved query lane and render the serial dropdown.
- [x] Run Node, Python contract/driver tests and Rust unit tests.

### Task 3: Packaged driver installation

Files: `App/src-tauri/src/host/driver_install.rs`, `host/service.rs`, `gui.rs`, `tauri.conf.json`, `App/drivers/newport/`, UI and tests.

Interface: `installDriver()` starts `install_driver` for fixed id `newport`; `driverInstallStatus()` polls `driver_install_status`. Return states `idle`, `running`, `completed`, `failed`, `unknown`, with `restart_required` and error message. No arbitrary paths/arguments accepted.

- [x] Add regressions for busy resources, modified package, duplicate start and installer return codes; observe failure.
- [x] Bundle pinned official MSI; implement maintenance gating, fixed system msiexec launch and original-job status; render only actionable missing/error states.
- [x] Verify package resources; build a separate candidate executable without replacing a running app. Node/Python/Rust checks passed.
- [x] Build and independent read-only review are complete. Publish this verified change on the assigned branch; the verified remote commit is reported in the final response.

### Additional requested workflow cleanup

- [x] Name first, model and serial underneath; hide identity strength.
- [x] Replace check-policy switches with interval and Refresh now; prioritize the selected refresh.
- [x] Hide Fiber setup entry when there are no eligible controllers or configured setups.
- [x] Cancel the wizard on backdrop click, including deferred cleanup after an in-flight test.
- [x] Notifications expire after five seconds; left click dismisses and right click copies.
- [x] Show progress and suppress repeated clicks during async actions.
- [x] Hide normal proof TTL/release instructions and remove false unknown warnings during Disconnect.

## Verification and review record

Full frontend suite: 245 passing. Rust library suite: 170 passing. Python App offline suite: 392 passing, plus 33 focused Newport/TLB tests. Host-process cases remain deferred while the actual lab Host is running; they must not displace it. A first broad runner mistakenly included inherited Host-process cases and hit the production owner guard; the corrected runner excludes those classes and preflight processes.

Independent read-only review found two issues, both repaired and independently rechecked: SDK-owned enumeration must not return cached serials as fresh, and uncertain installer exit must retain its admission fence. The selected-refresh priority regression also observed failure before its fix. No laser enabling, output-setting, or installer execution occurred during verification.

Build/package checks and Git publication are completed only when recorded in the accompanying execution results; no GUI visual or fresh-machine installer qualification is implied by these unit tests.

Settings follow-up: driver metadata loads automatically when opening Settings; installed state is concise, unavailable features are hidden, and Host maintenance controls are under Advanced. The regression observed failure before implementation. Final frontend suite: 245 passing.

Candidate build: `App/src-tauri/target/debug` is separate from the active `target/connection-fix/debug` binaries. Resource validation found 38 exact source-matched runtime resources including the pinned MSI, with no retired resources.

Final real checks: the updated Host scanned controller `22500001`. An authorized read-only connection matched TLB-6700 firmware 2.4 and head 6722-P serial 0953; the sampled wavelength was 1061.814 nm and sampled output state was off. The diagnostic client then confirmed all its resources released. No parameter/output commands or installer execution were performed. These are sampled observations, not a physical laser-emission measurement.

Actual ingress found a v3-to-internal-scheduler method alias omission. The complete finite worker-ingress test observed the same unknown-method error, then passed after mapping only scan_lasers to the reserved query lane. Focused scheduler/driver checks passed 57 cases, and the full offline Python App suite passed 392 cases. Independent review accepted the final scheduler/selection/Settings deltas.

Expired metadata is rechecked when Test Connection is clicked; it no longer leaves the button silently disabled. Expired save proof displays a short Test again message and still requires a new identity proof. Final frontend suite passed 245 cases. Installed driver metadata is checked automatically when entering Settings.

Final packaged GUI SHA-256: `F1385464E0B4AC7AF53AC8195415E2A053325B2D99C167CFE41BF41F2BA94C6F`. Host SHA-256: `920AA6E44616E193FA4144D3ADC242B3409C79E302A0DDEA142D34C51DADC744`. Final resource check passed 38 exact mappings. The old App/Host were closed normally after current read-only snapshots and confirmed resource release; the verified candidate was started and its Host health checked. No process was forcibly terminated.

GUI visual acceptance, a fresh-machine installer run, CH340/CP210x package support, and full Rust instrument execution remain unqualified or separate follow-up work. The current build still uses the VISA Python worker.
