# OSA and App Communication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Inline execution and the real-console design are already approved.

**Goal:** Finish the existing OSA read/display/archive workflow and connect Apps to additional owning Hosts without replacing local devices.

**Architecture:** Keep the real OSA driver and Host archive. Add native certificate-pinned TLS channels to the same Host session/lease/operation dispatcher. Pair explicitly in Settings; keep credentials native and DPAPI-protected. Route web actions by Host identity.

**Tech Stack:** Rust/Tokio/rustls (ring), rcgen; existing Python VISA driver; vanilla JavaScript/Tauri.

**Spec:** docs/superpowers/specs/2026-10-06-real-console-design.md, sections OSA, Durable data, Remote transport and Two-App acceptance. Operator reduced current scope to OSA and App communication; other devices and private-runtime packaging are not current deliverables.

## Global Constraints

- Only real devices; no instrument emulator or automatic measurement/settings changes.
- TLS even on loopback; listener disabled by default, explicit loopback/Tailscale IP only.
- Keys/credentials remain native, DPAPI-protected; unpaired peers cannot inspect or operate devices.
- Reuse typed commands, five bounded channels, existing lease/boot/revision fences and cleanup. No command replay or automatic control reacquisition.
- OSA existing trace reads preserve panel settings; scanning requires separate authorization.
- Targeted tests per change, one integration regression; no unrelated runtime or driver expansion.

## Review Focus

1. A remote certificate changes: fail before credential transmission (Task 1).
2. A channel attaches using another peer's session: reject without granting authority (Task 1).
3. Revocation or disabled listener while commands are active: close remote sessions and use existing cleanup (Task 2).
4. Two Hosts have identical device names/IDs: keys, events and exports retain owning Host identity (Task 3).
5. Large/incomplete spectra or reconnect: preserve exact samples/hash; never replay reads or label incomplete data complete (Task 4).

### Task 1: Native TLS, trust and peer-bound sessions

**Files:** App/src-tauri/src/remote.rs, remote_tests.rs, host/sessions.rs, Cargo.toml/Cargo.lock.
**Interfaces:** `RemoteStore::open(path, host_id)`, `status()`, `begin_pairing()`, `approve(id)`, `authenticate(peer, credential)`, `revoke(peer)`; `ClientSessions::join_peer(params, peer)` and `revoke_peer(peer)`; pinned `connect_tls(endpoint, fingerprint)`.

- [x] Write tests for DPAPI persistence, endpoint restrictions, pairing expiry/approval, invalid credential, peer-bound attachment and certificate mismatch.
- [x] Run native focused tests and observe missing implementation failures.
- [x] Implement TLS13 and atomic encrypted trust storage; six-digit 120-second pairing with owner approval, at most five failed attempts and sixteen peers.
- [x] Verify focused tests pass; commit this independently testable native boundary.

### Task 2: Owning Host admission and native GUI remote clients

**Files:** host/service.rs, gui.rs, main.rs, remote_gui.rs, host_client.rs.
**Interfaces:** generic `serve_channel(stream, core, channel, first)`; local-only remote settings/pairing/revocation commands; GUI `remote_pair`, `remote_connect`, `remote_call`, `remote_subscribe`, `remote_result`, `remote_disconnect`, `remote_peers`. Each remote client owns independent control/heartbeat/results/safety/events streams.

- [x] Test authenticated TLS admission into typed dispatcher, no local configuration/Host stop over remote, revocation and dropped channel cleanup.
- [x] Run tests RED, implement Host listener lifecycle and remote GUI client map; no raw SCPI/proxy and no machine-guard weakening.
- [x] Run tests GREEN and build both native binaries; commit.

### Task 3: Additive multi-Host web routing and Settings

**Files:** web/main.js, host-client.js, remote-client.js, console-ui.js, overview.js, setup.js; App/tests/remote*.test.mjs.
**Interfaces:** session `clientFor(hostId)`, `clients()`, `apply(event, remote)`, per-Host identities/resync. Archive/result fetches select the owning Host; local configuration stays local.

- [x] Test two distinct Hosts, duplicate device IDs, events, heartbeat/result/archive routing, cached remote offline rows and remote settings UI.
- [x] Observe RED; implement native pairing/connect/revoke Settings and additive Overview; keep simple Connect/Read trace/Save device workflow.
- [x] Verify web tests GREEN; commit.

### Task 4: OSA and communication integration

**Files:** Code/Debugs/check_osa_host.py (existing), a short remote acceptance diagnostic and docs/development.md.
**Interfaces:** existing OSA trace native descriptor and chunked archive API, operation IDs and hashes.

- [x] Run offline OSA/worker/archive, web and native regression once.
- [x] With applicable staged authorization, read the known real OSA's existing trace, display/export exact samples, verify panel settings preserved and release evidence.
- [x] Test TLS loopback with real Host data, peer rejection/revocation, observer/controller contention and owning-Host result agreement. Do not call a same-pipe test remote acceptance.
- [x] Record actual two-PC Tailscale verification as pending if no reachable paired peer; do not substitute a simulated device or claim campus network acceptance.
- [x] Review completed branch once, fix important findings with targeted regression, commit verified changes on PIC only. Do not merge main or 1060 branch.

Software handoff: native 176, web 230 and targeted VISA Python 88 tests passed. Both native candidates compiled. On 2026-10-06, the operator authorized the known OSA's existing-trace read/display/save stage, without scanning or panel changes. The fresh targeted VISA regression passed 88/88 before hardware access.

Acceptance evidence, recorded separately:

- The real AQ6370E returned Trace A with 2,000 native dBm samples. The native TLS loopback diagnostic verified identical local/remote archive bytes and manifest, local/remote control contention, certificate and unpaired-peer rejection, revocation, and verified normal-restart release recovery. Queried panel context before/after matched; sequential-read consistency remains explicitly unproven.
- The native App rendered the real spectrum, reloaded its historical archive after Host restart, and exported CSV with all 2,000 parsed samples exactly equal to the archived native values and matching metadata.
- Two ordinary native App windows attached to one local Host. The observing window automatically received the controller's new real trace with the same read timestamp, while its read/disconnect controls remained disabled. Closing the observer left the controller connected. Both windows then closed normally, and the owned Host reported resource release, confirmed successful worker exit and Host exit code 0.

The two ordinary windows used the local named pipe: this is local GUI synchronization evidence, not the prescribed paired network-only native GUI topology. That native TLS GUI pairing workflow and actual two-PC Tailscale acceptance remain pending operator/peer participation. The native TLS diagnostic does establish real remote data/ownership transport, but does not substitute for those GUI/topology checks. Installed/private-runtime qualification and other devices remain outside this increment.
