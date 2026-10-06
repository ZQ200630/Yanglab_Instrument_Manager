# Real Instrument Console

Status: approved by the operator's automatic-authorization instruction on 2026-10-06. Proceed through written plans and inline implementation without repeating equivalent approval prompts; physical safety boundaries still apply.

## Intent and scope

Yang LAB INSTRUMENT CONSOLE is a simple, English, light-theme desktop application for operating real laboratory instruments and retaining their data. Each computer owns its local hardware through its Host. A GUI also displays and operates explicitly paired remote Hosts' instruments without replacing its local instrument list. The authoritative Host publishes changes to every observing GUI.

The operator approved the conversational architecture on 2026-10-06: real-only execution, device-specific pages and storage, then encrypted remote connections. This document fixes the interfaces, safety constraints and acceptance criteria for that architecture. Approval of this document permits preparation of the implementation plan, not unapproved hardware actions.

Deliver in three ordered increments:

1. Real-only startup, registration and driver acceptance.
2. Device-specific pages and durable measurement storage.
3. Authenticated network transport and multi-Host GUI acceptance.

There is no cloud Hub, public listener, instrument emulator, fabricated inventory, or synthetic measurement backend. Unknown instruments previously excluded by the operator remain excluded. Hardware that is absent is reported unavailable, not accepted by substitution.

## Verified starting point

- The installed Host and worker were launched with the obsolete simulation flag even though saved startup preferences selected real hardware. Attaching an existing Host did not compare its runtime backend with the requested configuration.
- The current production application is local-only. A single named-pipe client and Host identity drive its GUI; Remote Hosts is a placeholder. Two GUIs using that one pipe are not remote acceptance.
- Real enumeration found GPIB0::4::INSTR, CH340 COM4, and CP210x COM5 with USB serial B0AFEF6C10A3ED11B81296412981D5C7. Serial-chip identity alone does not prove the attached controller's model or protocol.
- The separately authorized identity-only OSA probe returned YOKOGAWA, AQ6370E, 901C12907, 01.04, and confirmed release. It did not trigger a sweep, send ABORt, or alter measurement settings. This is not spectrum acquisition acceptance.
- Neither registered MDT serial was enumerated. A PM400 resource/sensor is not yet identified. USB instruments DS7A241400216 and MY52091763 and network resource 10.229.158.143 remain outside the current probe scope.
- Host result transport is bounded, boot-bound storage. Voltage/Gain plots are GUI memory; there is no complete durable recording workflow.

## 1. Real-only execution and migration

Remove the synthetic worker backend and both experiment simulator modules, their factories, fabricated probes/inventory, runtime modes and selectors, and simulator-dependent feature fixtures. The packaged application and experiment CLIs must contain no selectable or fallback synthetic-instrument path. Obsolete command-line flags fail closed. Disconnected startup is valid; fabricated readings are not.

The only instrument execution path uses the existing Code/Utils drivers, with Fiber motion routed through Code/Setups/fiber_coupling.py. Development and ordinary offline regression remain in Anaconda VISA. The operator subsequently selected an installed App with a private bundled runtime and clean Windows acceptance; see 2026-10-06-private-runtime-design.md. That deployment increment remains distinct from current runtime implementation and does not weaken hardware safety. The Host starts without opening instrument ports or restoring outputs. GUI startup automatically connects to its local Host; hardware connection remains an explicit device action. The Overview contains statuses, not Host configuration or Add New. Device Setup owns Add New; Settings owns persistent Host, remote and data-folder configuration.

An attached Host must advertise a compatible real-only protocol/backend. The new GUI must not attach instrument actions to an obsolete synthetic Host. Upgrade first obtains genuine lifecycle/resource-release evidence and shuts down the old Host normally; it does not silently kill an unknown owner. Saved settings alone cannot establish a different running backend.

Before tightening durable schemas, back up legacy preferences and the device registry atomically. Existing real-verified registrations retain their identity/revision-bound evidence. Any registration without real proof becomes inactive/unverified, retaining its editable name/model/connection parameters but no fabricated expected identity, proof, automatic-check consent or command authority. Archive obsolete drafts rather than approving them. Invalid records cannot make every unrelated device unusable. Migration is idempotent and never rewrites synthetic results as hardware results.

Preserve historical reports and data with truthful original provenance, and keep reference_code read-only. Historical provenance/import rejection is not an executable simulator. Retain protocol/parser/lease tests using bounded transport doubles; these are hardware-free safety checks, not alternate instruments or GUI data sources. Update active documentation and the obsolete AGENTS.md execution-flag rule, preserving its safety and pre-hardware verification requirements.

## 2. Identity, availability and ownership

Device keys remain Host ID plus device/setup ID. Persistent identity, model, profile, revision and probe-version fences remain mandatory. Real connection does not reuse an old synthetic verification report. Resource bindings are canonicalized and duplicates fail before opening. A Fiber setup and its member controllers cannot acquire conflicting physical ownership.

Availability distinguishes enumeration, recent successful communication, open lifecycle and fault. ONLINE is supported by fresh communication evidence; LOCAL/REMOTE describes ownership location, not safety or connection health. A stale remote or driver read is shown as stale/unknown, never a substituted zero. A failed transaction is not automatically replayed after reconnect.

Periodic connection checks are enabled only after identity-bound operator consent and only for profiles with read-only probing and no unverified open effects. Voltage/Gain and serial profiles with possible reset effects are not auto-opened at startup. Once explicitly connected, their normal monitoring uses the driver and existing watchdog/interlocks. MDT opening remains separately supervised where serial-control-line effects are unverified.

Leases retain Host boot, domain, revision, control epoch and session bindings. Connect obtains authority automatically when free; another owner produces a simple observing state, not a forced takeover. Diagnostics retains detailed cleanup evidence without crowding the normal page.

## 3. Device pages and driver completeness

All pages use a small static recognizable instrument icon, one Connect/Disconnect control, a concise status, and device-specific data/actions. No interactive 3D preview or simulation badges remain. Advanced/maintenance controls are collapsed.

### OSA

The main action is Read trace: retrieve an existing front-panel trace A-G without initiating, aborting, clearing or reconfiguring measurement. An explicit secondary Start sweep action is separate. A requested new bounded sweep is admitted only when the current panel sweep mode supports it; do not silently change REPEAT to SINGLE. Connect/read-only close preserves the panel's ongoing measurement state. Cancellation of a software-owned acquisition can abort only that explicitly initiated measurement under its declared lifecycle policy.

Read transfer format, level scale/unit, sample count and relevant measurement context through typed driver queries. Decode the current ASCII or binary format without issuing a format setter. Never assume every Y value is dBm. Preserve native values/units; convert only when physically meaningful and validated. The legacy power_dbm compatibility field is valid only for actual or correctly normalized absolute power, not arbitrary linear/density/math data. Reject unsupported or inconsistent interpretation before publishing a mislabeled spectrum.

Read the X/Y arrays with bounds, matched lengths, finite-value checks and monotonic wavelengths. Record the read interval, panel context before/after and consistency quality. Sequential reads during a changing front-panel trace cannot be claimed atomic; discard known inconsistencies and mark unproven consistency rather than stopping the instrument implicitly. Read time is not invented as the instrument's measurement time.

Show the spectrum, raw-sample cursor, actual axis units, source identity, capture time and capture list. Save/export preserves the exact samples and metadata; plot downsampling never changes stored data. Center/span/resolution/sensitivity remain front-panel-owned.

### Voltage Source

Show eight channels' measured voltage/current with freshness and quality, plus separate targets and last commands. A selected channel has an elapsed-time plot. Provide bounded Apply and All zero, with batch targets available without bypassing the driver. Connect/close disclose their all-channel-zero behavior and possible serial-open effects.

Keep 0-14 V, <=0.1 V normal steps and >=50 ms intervals. Handled faults/shutdown send all channels to zero immediately. A transmitted zero, fresh controller telemetry and a physical voltage measurement remain separate evidence.

### Gain Chip Driver

Show measured temperature, target, TEC/current enable states, confirmed current setpoint and stability/watchdog evidence. Do not label a confirmed current setting as an independently measured output current. Provide safe TEC/current controls, bounded current ramp through the driver and temperature/current recording. Existing PID operations belong in typed, explicitly gated advanced maintenance, not raw-command entry.

Keep 0-200 mA and 15-40 degC targets. Current enable requires TEC on and measured target +/-0.2 degC stability for five consecutive seconds. Fault/close disables current before TEC; no reconnect automatically re-enables either output.

### PM400

Center the page on sensor identity, current measured value/unit/time, measurement kind and relevant wavelength/range/averaging context, with a time plot and recording/export. Unsupported sensor functions remain capability-gated. Connection and normal close preserve panel settings. Calibration, zero, reset, detector-response and adapter-type changes remain explicit maintenance authorization, not connection acceptance steps.

### MDT693B and Fiber setup

Give MDT a useful read-only controller page: identity, actual XYZ voltages, limits, Master Scan, freshness, fault and authority state. It does not provide a second direct-motion UI; link to the Fiber setup.

Fiber cards use laboratory coordinates +X right, +Y away from operator, +Z up. Bind sides by the registered serials, never COM order. Left logical XYZ maps to MDT YXZ; right maps to MDT XYZ. Show observed voltage separately from session-only open-loop position estimate, baseline and nominal calibration consent. Save the motion/authority log. Single-sided setups remain valid when the known opposite controller is absent.

Keep the 75 V controller ceiling, <=0.1 V steps and >=50 ms intervals. Toward-chip moves are <=0.2 um; other per-axis moves <=1.0 um. Fault, authority loss or external motion holds position and invalidates estimates. No rollback or automatic zero; all movement stays in the setup layer.

## 4. Durable data

Add a Host-owned recording/archive service distinct from the existing bounded delivery-result cache. The owning Host records data once; GUI observers subscribe rather than polling/recording duplicates. Each archive lives under a configured Result root as Result/<name>/<recording-id>. Names are short, validated and cannot traverse paths. Default developer data remains in the workspace Result tree; installed Hosts persist a selected writable data root in Settings, not in a driver directory.

Store an immutable manifest, native spectra and append-only telemetry/motion events. Include Host/device identities, configuration revision, source kind, UTC and monotonic elapsed time, units, data quality, read interval, requested vs observed values and relevant panel context. Recorder operation IDs prevent duplicate writes. Archive references survive Host restarts but grant no live command authority. Exports include metadata alongside raw CSV values. Remote export copies verified data to the receiving computer without changing its provenance.

Large spectra use bounded paginated/chunked archive transfer with total length and content-hash verification, not an oversized single JSON result. Retain existing control-frame limits; do not raise them globally to fit an AQ6370E's maximum-size trace. A failed/incomplete transfer is never displayed or saved as a complete capture.

The main page offers Record/Stop and Save/export, with a concise destination. Default telemetry recording interval is one second, bounded below by actual driver cadence and existing scheduling capacity; do not increase serial traffic merely to draw a faster plot. Spectrum recording captures explicit completed reads/sweeps rather than silently triggering repeat scans. Recording is not experiment automation.

Use bounded queues and explicit backpressure. A disk-full/write failure ends recording visibly and marks a partial archive; it does not claim success or silently discard samples. A configured fail-safe lease revocation runs the driver's existing shutdown policy. Host-owned recordings stop and flush on owner release/loss, application shutdown, or Host stop; observing GUIs closing do not stop another owner's recording. Recovery marks interrupted records without restoring outputs. Historic data retrieval is read-only and not boot-authority-fenced.

## 5. Remote transport and multi-Host GUI

Use a Rust-owned TLS network transport and explicit certificate/peer authentication over the existing Tailscale network. This keeps pairing/configuration in the App. Alternatives considered: a Tailscale Serve proxy adds per-machine external setup; SSH tunnels add manual endpoint/credential management. Neither is required by this design. Do not alter the operator's Tailscale account, tailnet, sharing or access policy automatically.

The network listener is disabled until enabled in Settings. Bind only to an explicitly selected Tailscale interface/address or loopback; never default to wildcard, LAN/public exposure, or Funnel. TLS is required even for loopback acceptance. Use a persisted Host certificate/identity, pinned certificate fingerprint and per-peer random credential, protected using Windows credential/DPAPI facilities. Keys/tokens never enter frontend state, operation logs or URLs.

Pairing is an explicit, short-lived operator-approved flow: enter Host endpoint, compare fingerprint/short code, approve on the instrument-owning Host, then persist peer trust. Unpaired clients cannot enumerate devices, subscribe, read archives or execute. Pairing does not silently enable reciprocal listeners or grant a peer takeover capability. Each direction can be paired independently; either Host's local GUI still sees the authoritative state of its own hardware.

Keep local named-pipe SID/PID authentication intact. Remote TLS authentication creates a peer-bound logical session before admission to the same typed Host commands. Separate control, heartbeat, events, safety and result channels retain existing bounds/fences. A remote channel is not an arbitrary TCP-to-serial/SCPI bridge. Unsupported versions, oversized frames, mismatched identities, invalid tokens and revoked peers fail closed. Revoke closes attached channels and revokes affected leases through the existing cleanup path.

The native GUI maintains a client map keyed by Host ID and peer endpoint; the web session routes each action/result/event/heartbeat to its key's owning client. A remote event never overwrites the local Host identity. Overview combines local devices and instruments of connected paired peers. Disconnected peers' previously configured devices remain labeled REMOTE/OFFLINE or stale, not silently removed or shown as local. Duplicate names do not collide. Device Setup clearly separates local configuration from remote observation; local saves never rewrite a peer's instrument bindings.

When GUI A operates Host B's instrument, Host B performs the actual driver call and broadcasts the same revision/result to GUI A and GUI B. Only one controller owns a domain; other GUIs observe. Connection loss revokes that controller's lease: Voltage zeros, Gain current-off then TEC-off, piezo holds/invalidate. Unknown outcomes are reconciled using the operation ledger without repeating the command. No automatic control reclaim, movement or output restoration follows reconnect.

## 6. Two-App acceptance without fake devices

Normal use retains one hardware-owning Host per computer, protected by a machine-wide physical-owner guard. Do not weaken this guard to start two physical workers for acceptance.

For a single-PC network acceptance, use two GUI profiles and independent Host IDs/data directories. Host A is the sole real hardware owner. Host B is an explicit network-only profile with no physical worker, local probes, or device registration capability; it returns an empty local inventory and connects to Host A through TLS loopback. This is not an instrument simulator: every displayed device and sample originates from Host A's real driver. Network-only status is explicit and has no synthetic data path. The physical-owner guard remains unchanged for every hardware-capable profile.

Prove GUI B's network command reaches Host A, then verify identical live status, operation ID and sample hash in A/B. A local observer and remote controller must contend for the same real device lease. Repeat after controller disconnect, peer revocation and Host restart; verify stale data and unknown outcomes honestly. A reciprocal empty-Host connection can verify Host isolation, but cannot count as testing another physical device.

This establishes real network routing, authentication, data sharing and contention on one PC. A separate two-computer Tailscale acceptance is still required before claiming the actual laboratory network has been validated; it needs a reachable peer endpoint and its operator's pairing approval. Do not describe loopback results as proof of campus/Tailscale reachability.

## 7. Verification and hardware stages

Run hardware-free protocol/parser/security/lifecycle regression checks in Anaconda VISA before physical diagnostics, without a synthetic instrument backend. Native/web tests verify real-only startup, legacy-proof rejection, routing, framing, pairing, revision/lease fences, recording failures and shutdown. No test imports a shipped instrument simulator. Transport doubles must never escape into an instrument factory or production GUI.

Real hardware evidence is collected with Code/Debugs and retained in Result/console under short run directories. Every real-device check records identity, action, quality, failure/timeout and actual release evidence. Logs never substitute for physical measurement.

Each hardware stage requires its own applicable approval:

1. Enumeration: inspect VISA resources and USB/serial metadata without opening unknown devices.
2. Read-only connection: probe identified devices without output/measurement changes. Only the OSA *IDN? stage has been approved and completed in this run. Additional OSA metadata/trace queries need their own stated scope. Serial open/close effects must be disclosed rather than labeled read-only.
3. Action: separately approve OSA scanning, Voltage startup/close zero and any nonzero step, Gain interlock shutdown/TEC/current action, PM maintenance, or Fiber baseline/nominal/motion. Never infer action approval from this document or the broad goal.

For OSA, verify a real existing trace, exact sample export, units, format and panel preservation; a new sweep is a separate action. For Voltage, verify fresh eight-channel telemetry and release, then an approved <=0.1 V CH1 step and zero if authorized. For Gain, verify five readback fields and safe shutdown evidence, then only approved TEC/stability/current actions. PM400 and MDT/Fiber remain unavailable until the correct resource/serials appear and checks are authorized; they are not substituted or claimed hardware-passed.

## Completion criteria

- No simulation execution option, backend, fake inventory or synthetic measurement is packaged or reachable.
- Ordinary startup uses a compatible real-only local Host without opening hardware automatically. Legacy simulated authority cannot operate hardware, and user data is not destroyed or relabeled.
- Every presently connected, identified in-scope driver has a successful recorded real acceptance result for the approved functionality. An unresolved driver failure prevents completion and must be fixed or reported as pending; enumeration alone never counts. Physically absent or explicitly excluded devices are not falsely accepted.
- Each supported device has its appropriate page and truthful units/freshness/authority state. OSA data reads, plots and exports match real samples; durable recording survives page changes/restarts and reports incomplete writes.
- Paired remote devices route to their owning Host, update both GUIs and obey shared ownership/safety rules. True TLS two-App acceptance includes authentication rejection, contention, state/result agreement and loss/recovery evidence.
- The actual two-computer laboratory link is either tested with evidence or explicitly listed as awaiting a real peer; loopback is never mislabeled full laboratory acceptance.
- Delivered App binaries match the verified source. Preserve unrelated work. The operator's later release instruction authorizes publication of the verified common framework to the target repository's main only AFTER its acceptance, including the private-runtime clean Windows gate; then create codex/pic-desktop and codex/laser-1060-desktop from that same commit. Do not publish an incomplete baseline or automatically merge later instrument-branch changes.

## Primary references

- [Yokogawa AQ6370E Remote Control Manual, third edition](https://cdn.tmi.yokogawa.com/1/9667/files/IMAQ6370E-17EN.pdf): trace queries, level scale/units and current ASCII/binary transfer format. Its trace response depends on panel scale; the existing unconditional dBm label is insufficient.
- [Tailscale HTTPS guidance](https://tailscale.com/docs/how-to/set-up-https-certificates) and [Serve documentation](https://tailscale.com/docs/reference/tailscale-cli/serve): reviewed transport/proxy alternatives; no tailnet changes are part of this design.
- [tokio-rustls documentation](https://docs.rs/tokio-rustls/latest/tokio_rustls/) and [rustls documentation](https://docs.rs/rustls/latest/rustls/): candidate native TLS boundary. Dependency versions, crypto provider and available build tooling are to be pinned and verified in the implementation plan before installation.
