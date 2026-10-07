# Laser control and two-layer operating limits

## Result

Compact Laser Status precedes full-width Control. One state-labelled Enable/Disable button, native scan start/stop, and wavelength movement are primary controls. Fine tuning and operator limits are collapsed; serials and firmware are last. Ordinary controls take Remote and return Local after acknowledgement. Front-panel changes appear through foreground read-only refresh; inputs survive updates. Automatic and requested reads show progress and suppress duplicate requests.

The shared manufacturer table resolves standard heads and the documented permanent fiber-coupling -P option. The attached 6722-P has a published envelope of 1045–1085 nm and maximum 10 nm/s. Unknown/custom head suffixes remain read-only. A fresh controller MAXVEL query also caps each scan. Operator limits can narrow the hardware range or lower maximum speed, never expand either. The same speed ceiling applies to travel to Start and return. Limits are persisted and activated through preserving disconnect/reconnect.

Native scanning verifies all settings before START, performs one forward/return cycle and preserves reverse-output blanking. STOP holds the current position; it never sends RESET or disables output. Start never enables emission. Wavelength movement enables motor tracking; Piezo is cavity fine tuning in percent; Tracking changes motor following, preserving output. No cleanup write or action replay follows an uncertain command.

## Review and offline validation

Review identified two activation defects: worker physical-rebinding validation rejected legitimate narrower limits, and connection admission could overlap a saved but unactivated configuration. A failing reconnect regression was corrected with a narrowly allowed limit-only reconfiguration. Saving now shares the ordinary admission lock until activation, and failed activation blocks that domain until Host restart reloads durable limits. Regression tests cover ordering, failure retention and durable save/reopen without identity changes. An actual native Host with bounded injected transports also exercised save, configuration activation and read-only reconnect at the new revision.

Final offline results:

- VISA Python driver/protocol/lifecycle suite: 1040 passed.
- App Python suite against rebuilt native Host: 521 passed.
- Native Rust driver and protocol: 23 + 9 passed.
- App Rust library: 179 passed.
- Node frontend: 263 passed.
- Isolated Edge acceptance: compact status, wider controls, one output action, input preservation, typed scan/stop/tuning, busy stop, save/reconnect, visible automatic refresh, duplicate suppression and 800/960/1440 px layouts passed. Existing read-only status acceptance passed.

An obsolete -P unknown-envelope assertion initially failed in the full App suite. Its expected result was updated to the documented automatic limits; the complete 521-test suite then passed. A bounded browser fixture initially omitted the disconnect epoch increment and was corrected to respect the production context fence. These failures remain in the work/tool records; the earlier full-suite log is retained under Result.

## Build and desktop delivery

Offline locked MSVC builds succeeded for native TLB, Host and desktop. The package checker matched 42 resources, including the shared model JSON and loader, with no retired resources. Artifact SHA-256:

- GUI: 59B5AB5136A75E18CA0CD15B6F51F840E1FDD3C2C324A3C616202E326837CD20
- Host: B8D572915853481985E651DE4C0674C788D368D3ADC8786BA6210D0B84AE5763
- Packaged native TLB: 22BEF69A6D80F3683EB4E25B50D7123F2A5D3675473B1BEE3660E7734B1A0EBC

Using the user's existing authorization to disconnect/update, the former session was normally disconnected, the GUI closed normally, and Host stop separately confirmed release and successful worker/Host exit. No real owner was force-killed. The updated desktop was reopened and authenticated metadata confirmed startup_error=null, the original registered device retained and disconnected. The desktop still uses the existing VISA Python scheduler; this work does not claim the entire application is Python-free.

## Real read-only qualification

After all offline suites passed, the rebuilt Host acquired two identity-bound read-only samples from controller 22500001, firmware 2.4, head 6722-P, head serial 0953. Both samples reported output disabled. Automatic hardware/operating range was 1045–1085 nm and published maximum speed 10 nm/s. No setpoint, scanning, output-enable, Tracking or Local/Remote command was sent. Normal cleanup confirmed resource release, worker exit code 0 and Host exit code 0. Controller reports are not independent optical measurements.

Measured connect terminal time was 5.698 s, followed by fresh metadata publication after 0.013 s. One status read was 2.835 s with publication after 0.016 s. SDK packet pacing and the 13-query status sequence are unchanged. Real output/tuning/scanning, scan continuity after returning Local, and physical start/stop latency are not qualified by this read-only run. They require the separately authorized reversible-action stage.

Evidence remains under ignored Result/laser-control: checks, hardware/run.json, ui/laser-control.png, preserving-disconnect.json, host-stop.json and desktop-start.json. Historical failure evidence was preserved.

Sources and operating semantics are linked in docs/tlb6700.md.


## Final revision: target editing and latency

The earlier hashes/counts above describe the first desktop revision. Latest source adds fixed aligned target digits, left/right selection and up/down tuning, newest-value coalescing, Tracking-preserving Ready target writes, independent Forward/Backward velocities, target disabled through scanning and actual stopped wavelength adoption. Velocity inputs stay local until verified Start.

Frequent Target/Piezo and Scan Start/Stop finish their worker operation on ACK. A separate motion observation updates wavelength/setpoint/Tracking/completion; power/current/output retain the full sample and original timestamp. Full connect, explicit refresh and output/Tracking readback remain. Prestart verifies all five scan settings, fresh controller MAXVEL, immutable head envelope and narrower operator limits. ACK is not arrival or optical output proof. Motion now checks OPC before/after wavelength, so a busy-to-complete transition requires another sample before adopting a final stop position.

The latency review found two important defects, both fixed with bounded regressions: cancel during debounce left Updating stuck after reconnect; reading wavelength before a completed OPC could adopt an in-motion wavelength as the stop target. Queue busy callbacks are scoped to their specific queue, connect resets sending state, and motion completion brackets wavelength. No hardware action was used for those regressions.

Offline results: driver Python full suite 1044 passed, followed by the added completion-transition regression and 27 focused driver/App checks passing; App Python 523 passed; native Rust 26 driver + 10 protocol passed; App Rust 179 passed; Node 268 passed; isolated Edge readings/control acceptance passed (keyboard coalescing, independent velocities, Stop/target behavior, saved-limit reconnect, automatic refresh and 800/960/1440 px controls). Rebuilt Host finite transport acceptance: 5 passed. Build/package verified 42 matching resources, no obsolete resources. Existing three Rust warnings remain.

An initial App Rust build encountered a locked running GUI resource, and an initial Host rebuild encountered a test-owned running Host binary. Both completed after normal closure/test exit; no forced process termination. All original failed build outputs and preserving release attempts remain in Result/laser-control/checks and latest-disconnect/latest-host-stop.json.

Final GUI SHA256: A143B18575495F97A0CD37D10352479C55E2A5A16A2D4BDAED1E90183B537CCA
Final Host SHA256: 652E8917F0193C9D6AE6D08E5233915914AB2A962B93CB1174FCC1A0F55123C2
Final native driver SHA256: AEDB8E7CC61D103B10466DBD81D6D03776E6BC70CAB4B45C2641B36EE845D882

### Real read-only timing

Evidence: Result/laser-control/fast-read-hardware/run.json, through rebuilt native Host. Bound identity is controller22500001 / firmware2.4 / head6722-P serial0953. Initial allowlisted getter pacing changed from200 to10 ms; retry and setter pacing remain200 ms. Sent setters are never replayed. Five complete reads and twenty five-getter motion reads passed. All five complete samples reported output disabled; this is controller telemetry, not independent optical measurement. Preserving cleanup confirmed resource release and normal worker/Host exit0.

Connection terminal: 2.4513 s (previous same-controller revision 5.698 s); metadata publication after terminal 0.0189 s. Native full-read intervals: 0.2100–0.2151 s (previous about2.7 s). Native motion intervals: 0.0769–0.0801 s. End-to-end Host read operations including receipt polling: full0.3619–0.3742 s; motion0.1943–0.2287 s, then metadata publication13–23 ms. The diagnostic polls at50 ms; these are not exact GUI click-to-paint times. SDK initialization/drain still contributes connection overhead and is not separately instrumented.

By command count, Target takes two getters plus up to three setters; Stop one getter plus up to three setters. Remaining configured pacing is roughly0.62/0.61 s before normal SDK/Host overhead, plus a separate approximately0.078 s motion sample. Start deliberately retains five-setting write/verify and up to eight setters (about1.6 s of setter pacing). These are code-derived budgets, not measured setter/Start/Stop timing or physical motor settling. No tuning, output change or scan diagnostic was executed. Further setter pacing reduction requires its separately authorized action qualification.

The updated desktop was reopened normally, startup_error=null and original device registration/config_rev1 retained. Result/laser-control/latest-desktop-start.json records the disconnected initial instrument state. No new registration or physical scan was created.
