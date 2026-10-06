# Connections and request/approve pairing

Operator update (2026-10-06): this document records the original comparison-based design. Its displayed-number/manual-comparison requirement is superseded by the approved internal-lab flow **Request connection → Approve** with no number. Already trusted computers use ordinary **Connect** without reapproval. First-use trust now depends on the operator's approval, not independent identity comparison. All other transport, persistence, pin-mismatch, lifecycle, admission and device-boundary constraints remain; native v2 metadata is retained for compatibility. Follow the current workflow in `docs/development.md`.

Status: approved by the operator on 2026-10-06 ("认可"), including the first-pairing displayed comparison number. Implementation awaits review of the matching plan; the earlier two-tab UI layout is also approved.

## Intent and scope

Make computer pairing feel like an ordinary connection request, not a security configuration form. The operator requested **Control this PC** / **Control other PCs**, no manual full fingerprint or long ID, and **Request connection → Approve**. Keep the white English UI, additive local/remote device inventory, one physical owning Host, existing OSA data workflow and instrument safety boundaries.

Work only on `codex/pic-desktop`. No hardware operation, dependency installation, cloud account, automatic peer discovery, Tailscale/firewall changes, private-runtime qualification or merge into main/1060 is included. An IP address remains necessary to identify a new computer; automatically discovering all lab PCs is a different feature.

## Chosen interaction

- **Settings → Connections → Control this PC** shows Allow connections, the configured address, pending connection requests and authorized computer cards. Listener configuration is collapsed after setup. A pending card has the requesting computer's name, observed source IP, a six-digit comparison number and **Approve / Reject**. No owner-side Open pairing or code generation step is needed in the new flow.
- **Control other PCs** shows paired computer cards with name, address, online state and one Connect/Disconnect action. **Add PC** opens an address field; port defaults to 9443, and an optional nickname is under Details. **Request connection** opens a waiting card with the same six-digit comparison number, a 120-second deadline and Cancel.
- The comparison number is displayed, not entered or copied. On first pairing, the operators compare the two screens through a trusted channel before Approve. No additional confirmation checkbox or typed code is introduced. This is the only first-pairing identity check visible in the normal flow.
- After approval, the native client stores the owning certificate and credential, closes the temporary pairing connection and connects through the existing pinned authenticated channels. No instrument is opened, no control lease acquired and no previous instrument command replayed merely by pairing or reconnecting.
- Full certificate fingerprints and internal IDs are read-only Details/Diagnostics information, not fields in the normal form. Revoke/Forget are under computer details, not next to every routine Connect button. Credentials and private keys never enter web state.
- Network-only acceptance profiles may control other PCs but cannot enable a local listener. Local Host/general settings remain under Settings; Overview receives no new networking buttons.

The two tabs and all pairing fields retain accessible labels, keyboard navigation, visible error/wait states and responsive spacing. Host refresh/events must not erase an address draft, switch tabs or hide an active request.

## Alternatives and trust trade-off

1. **Recommended: request/approve plus a displayed comparison number.** Removes manual long strings and input codes while retaining a practical first-pairing check. Approval alone is not proof of identity: names are caller-supplied and an IP address by itself is not a certificate identity. The operators must actually compare the number.
2. **Approve-only first-use trust.** One fewer thing to look at, but without an independent comparison it cannot detect an attacker impersonating either side during the first connection. Do not silently replace the existing fingerprint check with this weaker guarantee.
3. **Keep manual fingerprint and input code.** Smallest backend change, but it does not meet the operator's requested workflow.

The recommendation is not an identity directory, account login, enterprise PKI or a formal proof against every attacker. A six-digit comparison is probabilistic and requires honest operator comparison; its effectiveness depends on bounded attempts. The Tailscale IP range restriction does not verify tailnet membership or node ownership, so this App must not label names/IPs as independently verified Tailscale identities.

## Native protocol boundary

Reuse the current Rust TLS 1.3/ring implementation and DPAPI trust stores. Existing paired clients continue to use `connect_tls` with their saved full SHA256 pin; never add an unpinned fallback to device/authentication traffic. A changed saved certificate fails closed and requires deliberate recovery, not automatic re-pairing.

Add a versioned, unauthenticated **pairing-only** exchange on a dedicated TLS connection. It may expose only protocol version, the participating computer names/IDs, a request nonce/ticket, deadline and pairing state. It cannot inspect instrument inventory, retrieve archives, start a worker, create a client session or dispatch a device operation.

The first-pairing TLS verifier accepts an as-yet-untrusted certificate only for this isolated exchange, while still verifying the TLS handshake signature and restricting endpoints to the existing explicit loopback/Tailscale addresses. The requester obtains the actual presented certificate in native memory. This handshake alone authenticates neither computer to the operator; the comparison step supplies the first-use check.

Both native endpoints derive a 32-byte RFC 9266 `tls-exporter` channel binding from the completed, non-resumed TLS 1.3 handshake. Build the displayed comparison value from SHA256 of a domain-separated, length-delimited transcript containing that binding, protocol version, random request nonce, ticket, requester ID, owner ID and the presented owner certificate digest. Format the first four digest bytes, interpreted big-endian modulo 1,000,000, as six decimal digits. TLS exporter material is never sent to the web frontend and is not used as a credential or encryption key. The two endpoints independently compute their own number; the requester must not simply display a server-supplied number.

One pairing exchange per TLS connection; close it on every terminal outcome. Disable TLS resumption and 0-RTT for this bootstrap path. The saved certificate digest must exactly match the certificate used in this approved exchange, and the saved owner ID must match its transcript. Never trust a fingerprint string supplied by an unauthenticated message instead of the actual certificate. An already pinned owner uses the saved verifier even on a new pairing request; bootstrap certificate acceptance is limited to an owner that has no saved trust.

Expose native `remote_pair_request(endpoint, nickname?)`, `remote_pair_status(request_id)` and `remote_pair_cancel(request_id)`. The first returns a public request handle and comparison/deadline information while a native-owned task waits for approval; status returns Waiting/Completed or a terminal error without credentials. The owning local `remote_status` adds request source/comparison/deadline fields; local `remote_approve(id)` is retained and `remote_reject(id)` is added. The versioned wire exchange uses a separate `remote_pair_v2` branch, an initial public acknowledgment and a terminal reply bound to the same request; it is not admitted into the normal authenticated dispatcher. Both wire participants validate message version, ID and phase, not just JSON shape.

The request state machine is **Waiting → Approved → Completed**, or **Waiting → Rejected / Cancelled / Expired**. Local owner approval is bound to the exact live ticket, nonce, listener generation and TLS connection. A reconnect creates a new request and comparison value; approval from an earlier connection cannot be transferred. Native cancellation on App close, owner listener disable/change and Host stop invalidates the pending ticket and drops the bootstrap connection. Cancellation racing with completed owner approval may leave an owner-side authorization: return that uncertainty honestly rather than claim all trust was removed, and use the explicit orphan-removal path below.

No credential exists before explicit approval. The owner atomically saves the approved peer credential under the existing trust lock before delivering it over that same TLS connection. The requester validates the reply's request/owner binding and saves the peer atomically with DPAPI before ordinary connection. If delivery/client persistence fails, report incomplete pairing; do not claim the requester is connected, replay approval or replace an existing trusted peer. The owner can remove the orphaned authorization explicitly through Details.

An unauthenticated request cannot rotate an already trusted peer ID or replace a saved owner pin. Existing authorization/release liability must be reconciled before an operator deliberately removes/replaces that identity. New anonymous bootstrap requests for an already authorized peer are rejected; ordinary authenticated reconnect remains unchanged.

## Bounds and failure behavior

- Keep the current 65,536-byte frame bound, TLS/first-frame 10-second deadlines, at most sixteen trusted peers and existing total channel semaphore.
- New bootstrap requests have a 120-second total deadline, at most four live pending requests, one live request per requester ID, at most five accepted starts per observed source IP per 120 seconds, and at most twenty accepted starts globally per 120 seconds. Rate-limit bookkeeping is bounded to 32 source entries; if full with unexpired entries, reject new sources instead of allocating indefinitely.
- Remove expired/disconnected requests promptly. Pending bootstrap work has its own four-slot semaphore so it cannot consume all ordinary authenticated channel capacity. Do not create instrument/control state for pending peers.
- Reject, cancel, expiry and listener/Host generation changes produce explicit terminal errors, discard temporary bootstrap state and never persist new requester-side trust. If native requester persistence succeeded but subsequent ordinary connection fails, show Paired / Offline; Connect is a separate non-replaying retry.
- Do not replace or erase existing trusted peers on a malformed frame, unknown protocol, duplicate request or failed disk write. Limit names to the existing 128-byte bound and validate native IDs/nonces independently of UI input. Never log credentials, private keys or exporter material.
- Old paired records remain usable. Retain the existing manually pinned/code-gated pairing method only as a versioned compatibility path for existing diagnostics/older clients; do not expose it as the new normal UI. It must not bypass the old owner-open/code/approve requirements. A new client facing an old Host reports that the Host needs updating, not a silent downgrade to unverified trust.

## Files and verification scope

Expected changes: `App/web/setup.js`, `console-ui.js`, focused connection rendering/state/styles, and `App/tests`; native `remote.rs`, `remote_gui.rs`, `main.rs`, the pairing branch of `host/service.rs`, and focused remote tests. Split the bootstrap exchange into a small module if needed rather than expanding unrelated GUI/Host code. Existing typed device commands, leases, archive/result/event routing, cleanup and OSA driver remain unchanged.

Test-first implementation must cover: no credentials/device access before approval; identical comparison values on one real TLS connection and different values across independently terminated connections; ticket/generation/nonce replay rejection; wrong certificate after pairing; reject/cancel/expiry/duplicate/rate bounds; incomplete trust persistence; already-trusted identity replacement rejection; normal request/approval and encrypted restart; old paired-record compatibility; native App close while waiting; and the two-tab UI without fingerprint/code inputs or erased drafts. Use actual native TLS for bootstrap integration, not a frontend simulator.

Then run one full affected web/native regression and the existing hardware-free Host communication diagnostic. Native GUI acceptance must be accurately recorded: the operator performs any security/access approval that desktop automation cannot perform. No OSA re-read is needed to accept this pairing/UI-only change. Actual two-PC Tailscale acceptance remains separate from loopback and pending until a reachable authorized peer is available.

## Existing evidence preserved

The current real OSA workflow is already accepted separately: genuine paired native TLS GUI loopback read, automatic local observer synchronization, exact archived 2,000-point dBm capture and normal resource/worker/Host release on 2026-10-06. Earlier native CSV/sample agreement remains separate evidence. Do not overwrite those reports or claim that this proposed pairing change has been implemented or accepted.

## Primary implementation references

- [RFC 9266: Channel Bindings for TLS 1.3](https://www.rfc-editor.org/rfc/rfc9266.html), sections 2 and 4: exporter channel binding, uniqueness and one exchange per TLS connection. It specifies the binding, not this App's proposed six-digit approval protocol.
- [rustls 0.23.45 connection implementation](https://docs.rs/crate/rustls/0.23.45/source/src/conn.rs): exporter API in the already pinned dependency. No new TLS library is proposed.
