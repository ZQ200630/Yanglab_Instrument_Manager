# Independent asynchronous requests

> Execute inline with superpowers:executing-plans; one independent review at the end.

The user wants long operations to run asynchronously while the interface remains responsive. JavaScript promises, Rust async IPC and bounded worker actors already exist. The remaining observed queue coupling is in `gui.rs`: connection tests/scans/refresh use the ordinary control pipe; operation polling/snapshots use the large-result pipe. Each pipe deliberately serializes framed request/reply traffic. Keep that serialization and give background tasks and lightweight status queries independent authenticated channels instead of removing locks or replaying calls.

Requirements: seven bounded channels per logical client (control, background, status, results, heartbeat, events, safety); new background/status channels attach to the same existing authenticated session. Status is read-only and cannot obtain control or execute commands. Losing background transport revokes the session, preserving unfinished probe responsibility; losing status transport behaves like losing result transport. Existing old clients keep their existing named channel capabilities. An old Host that rejects a new channel produces a clear upgrade/restart error. Shared SDK/global query and per-device sequencing protections remain unchanged. No hardware writes or automatic request retries.

Files: `App/src-tauri/src/{gui.rs,host_client.rs}`, `host/{sessions.rs,service.rs,ipc.rs}`; offline tests there and real-Host process fixtures in `App/tests`. No new dependency or GUI backend selector.

Ruling: recheck the transport failure fence after acquiring its mutex. Investigation found that a call queued before an earlier failure could otherwise write onto the invalid framed stream. Pin the race with a finite named-pipe test; never replay requests.

- [x] RED: slow background named-pipe request must not delay prepare or a lightweight operation query; slow file request must not delay snapshot/query. Keep safety/heartbeat separate.
- [x] RED: seven channels authenticate to one session, reject duplicates/foreign tokens, preserve sixteen-client limit; status cannot execute/acquire/install; status closure does not revoke, background closure does.
- [x] Implement background/status routing, strict Host allowlist and bounded pipe capacity. Upgrade-required error on unsupported channel attachment.
- [x] GREEN: full Rust library tests, existing frontend tests/browser acceptance and offline real-Host process tests; exercise actual server channel permissions using injected worker transport, never hardware.
- [x] Independent review, one RED/GREEN fix pass as needed.
- [x] Normally close the disconnected GUI and safely stop the released Host, rebuild matching binaries/resources, reopen, verify healthy read-only startup and new channel authentication.

Delivery uses the user's assigned `codex/laser-1060-desktop` branch; commit and push after verification.

Native hardware ownership and safe cleanup are preserved. No output action, tuning, scan diagnostic, installation or forced process termination is part of validation.
